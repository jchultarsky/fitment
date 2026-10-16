//! The shortlist, level 2 (`docs/research/m4-match.md`, SL1 to SL6): does
//! a candidate's signature hold every element a socket's demands, and every
//! pair of them in the relation the socket's stand in? A necessary
//! condition only: it keeps or drops, never passes. Section 6.4 proves it
//! never drops a candidate verification passes.

use serde::Serialize;

use super::MAX_SHORTLIST_CHECKS;
use super::signature::{
    EXACT, PartSignature, ReqLine, SigStack, SocketSignature, Window, angle, line_angle, point_line,
};
use crate::features::{FeatureId, Sense};
use crate::socket::geo::{V, add, dot, scale, sub};
use crate::verdict::RequirementId;

/// A candidate the shortlist keeps (SL6): for each requirement, the
/// candidate elements that may be its witness, which alignment starts from.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Kept {
    /// Each requirement's possible witnesses.
    pub domains: Vec<(RequirementId, Vec<FeatureId>)>,
    /// True if the budget ran out and the candidate was kept untested
    /// (SL4, `shortlist_budget`).
    pub budget_exhausted: bool,
}

/// What the candidate lacks (SL6), in the socket's terms.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Missing {
    /// The requirements it concerns.
    pub requirements: Vec<RequirementId>,
    /// What was not found.
    pub element: MissingElement,
    /// The window it was looked for in, where one applies.
    pub window: Option<Window>,
}

/// What kind of element was not found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MissingElement {
    /// No stack of the sense with a diameter in a span's window.
    Diameter,
    /// No stack with the openings required.
    Openings,
    /// No stack long enough.
    Extent,
    /// No plane with the area required.
    Plane,
    /// No cone of the angle, sense and range.
    Cone,
    /// No pair of witnesses in the relation the requirements' are.
    Pair {
        /// The other requirements of the pair.
        with: Vec<RequirementId>,
    },
    /// Too few distinct witnesses for distinct requirements (SL5).
    Distinct,
}

/// A candidate stack with its neighbourhood (SL2): the stacks of its sense
/// whose axes lie within `W_L` and `W_A` of its own, not transitively.
struct Cluster {
    feature: FeatureId,
    sense: Sense,
    point: V,
    dir: V,
    diameters: Vec<f64>,
    lo: f64,
    hi: f64,
    openings: u32,
}

impl Cluster {
    fn mid(&self) -> V {
        let m = if self.lo.is_finite() && self.hi.is_finite() {
            f64::midpoint(self.lo, self.hi)
        } else {
            0.0
        };
        add(self.point, scale(self.dir, m))
    }
}

/// SL4's count of comparisons.
struct Budget(u64);

impl Budget {
    /// Spends `n`; false once past the limit.
    fn spend(&mut self, n: usize) -> bool {
        self.0 = self.0.saturating_add(n as u64);
        self.0 <= MAX_SHORTLIST_CHECKS
    }
}

/// The kinds of required element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Line,
    Seat,
    Cone,
}

/// The state of one test: the windows, the candidate's clusters, and each
/// requirement's domain.
struct Test<'a> {
    socket: &'a SocketSignature,
    part: &'a PartSignature,
    clusters: Vec<Cluster>,
    wl: f64,
    wa: f64,
    lines: Vec<Vec<usize>>,
    seats: Vec<Vec<usize>>,
    cones: Vec<Vec<usize>>,
    /// SG5's groups: requirements one candidate element may serve
    /// together.
    line_groups: Vec<usize>,
    seat_groups: Vec<usize>,
    budget: Budget,
}

/// SL1 to SL6: the candidate kept, with its witnesses' domains, or what it
/// lacks.
///
/// # Errors
///
/// The first element, pair or matching not found, as [`Missing`].
pub fn shortlist(socket: &SocketSignature, part: &PartSignature) -> Result<Kept, Vec<Missing>> {
    let lever = socket.lever_mm.max(part.lever_mm());
    let wl = socket.linear_window(lever);
    let wa = socket.angular_window() + 1e-9;
    let mut test = Test {
        socket,
        part,
        clusters: Vec::new(),
        wl,
        wa,
        lines: Vec::new(),
        seats: Vec::new(),
        cones: Vec::new(),
        line_groups: Vec::new(),
        seat_groups: Vec::new(),
        budget: Budget(0),
    };
    let (nl, ns) = (socket.lines.len(), socket.seats.len());
    if !test.budget.spend(nl * nl + ns * ns) {
        return Ok(test.untested());
    }
    (test.line_groups, test.seat_groups) = socket.groups(wl, wa);
    let Some(clusters) = clusters(part.stacks(), wl, wa, &mut test.budget) else {
        return Ok(test.untested());
    };
    test.clusters = clusters;
    match test.domains() {
        Prune::Missing(missing) => return Err(vec![missing]),
        Prune::Exhausted => return Ok(test.untested()),
        Prune::Done => {}
    }
    match test.prune() {
        Prune::Missing(missing) => return Err(vec![missing]),
        Prune::Exhausted => return Ok(test.kept(true)),
        Prune::Done => {}
    }
    if let Some(missing) = test.distinct() {
        return Err(vec![missing]);
    }
    Ok(test.kept(test.budget.0 > MAX_SHORTLIST_CHECKS))
}

/// SL2's neighbourhoods; `None` once the budget is spent.
fn clusters(stacks: &[SigStack], wl: f64, wa: f64, budget: &mut Budget) -> Option<Vec<Cluster>> {
    let mut out = Vec::new();
    for anchor in stacks {
        if !budget.spend(stacks.len()) {
            return None;
        }
        let mut cluster = Cluster {
            feature: anchor.feature,
            sense: anchor.sense,
            point: anchor.axis.point_mm.to_array(),
            dir: anchor.axis.direction.to_array(),
            diameters: Vec::new(),
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
            openings: 0,
        };
        for other in stacks.iter().filter(|s| s.sense == anchor.sense) {
            if let Some((from, to)) = near(&cluster, other, wl, wa) {
                cluster.lo = cluster.lo.min(from);
                cluster.hi = cluster.hi.max(to);
                cluster.diameters.extend(other.diameters_mm.iter().copied());
                cluster.openings += u32::from(other.openings);
            }
        }
        out.push(cluster);
    }
    Some(out)
}

/// `other`'s stations on `cluster`'s axis, if its axis lies within `W_L`
/// and `W_A` of it; an unknown side unbounded.
fn near(cluster: &Cluster, other: &SigStack, wl: f64, wa: f64) -> Option<(f64, f64)> {
    let (origin, axis) = (
        other.axis.point_mm.to_array(),
        other.axis.direction.to_array(),
    );
    if line_angle(cluster.dir, axis) > wa {
        return None;
    }
    let ends = other
        .extent_mm
        .map(|t| t.map_or(origin, |t| add(origin, scale(axis, t))));
    if ends
        .iter()
        .any(|x| point_line(*x, cluster.point, cluster.dir) > wl)
    {
        return None;
    }
    let along = dot(axis, cluster.dir).signum();
    let station = |t: Option<f64>, low: bool| match t {
        Some(t) => dot(sub(add(origin, scale(axis, t)), cluster.point), cluster.dir),
        None => {
            along
                * if low {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                }
        }
    };
    let (from, to) = (
        station(other.extent_mm[0], true),
        station(other.extent_mm[1], false),
    );
    Some((from.min(to), from.max(to)))
}

/// How pruning ended.
enum Prune {
    Done,
    Exhausted,
    Missing(Missing),
}

impl Test<'_> {
    /// Every candidate element in every domain: what an untested keep
    /// offers alignment.
    fn untested(&self) -> Kept {
        let stacks: Vec<FeatureId> = self.part.stacks().iter().map(|s| s.feature).collect();
        let planes: Vec<FeatureId> = self.part.planes().iter().map(|p| p.feature).collect();
        let cones: Vec<FeatureId> = self.part.cones().iter().map(|c| c.feature).collect();
        let mut domains = Vec::new();
        for l in &self.socket.lines {
            domains.extend(l.ids.iter().map(|id| (id.clone(), stacks.clone())));
        }
        for s in &self.socket.seats {
            domains.extend(s.ids.iter().map(|id| (id.clone(), planes.clone())));
        }
        for c in &self.socket.cones {
            domains.push((c.id.clone(), cones.clone()));
        }
        Kept {
            domains,
            budget_exhausted: true,
        }
    }

    /// The domains as they stand.
    fn kept(&self, budget_exhausted: bool) -> Kept {
        let mut domains = Vec::new();
        for (l, d) in self.socket.lines.iter().zip(&self.lines) {
            let f: Vec<FeatureId> = d
                .iter()
                .filter_map(|&k| self.clusters.get(k))
                .map(|c| c.feature)
                .collect();
            domains.extend(l.ids.iter().map(|id| (id.clone(), f.clone())));
        }
        for (s, d) in self.socket.seats.iter().zip(&self.seats) {
            let f: Vec<FeatureId> = d
                .iter()
                .filter_map(|&k| self.part.planes().get(k))
                .map(|p| p.feature)
                .collect();
            domains.extend(s.ids.iter().map(|id| (id.clone(), f.clone())));
        }
        for (c, d) in self.socket.cones.iter().zip(&self.cones) {
            let f = d
                .iter()
                .filter_map(|&k| self.part.cones().get(k))
                .map(|c| c.feature)
                .collect();
            domains.push((c.id.clone(), f));
        }
        Kept {
            domains,
            budget_exhausted,
        }
    }

    /// SL2: each requirement's domain; the first empty one as missing.
    fn domains(&mut self) -> Prune {
        for r in &self.socket.lines {
            if !self.budget.spend(self.clusters.len()) {
                return Prune::Exhausted;
            }
            let domain: Vec<usize> = (0..self.clusters.len())
                .filter(|&k| self.clusters.get(k).is_some_and(|c| serves(c, r, self.wl)))
                .collect();
            if domain.is_empty() {
                return Prune::Missing(self.why_no_line(r));
            }
            self.lines.push(domain);
        }
        let planes = self.part.planes();
        // Each plane's known area with its near planes', once: it does not
        // depend on the seat.
        let mut areas = Vec::with_capacity(planes.len());
        if !self.socket.seats.is_empty() {
            for k in 0..planes.len() {
                if !self.budget.spend(planes.len()) {
                    return Prune::Exhausted;
                }
                areas.push(self.area_near(k));
            }
        }
        for s in &self.socket.seats {
            if !self.budget.spend(planes.len()) {
                return Prune::Exhausted;
            }
            let domain: Vec<usize> = (0..areas.len())
                .filter(|&k| areas.get(k).is_some_and(|a| *a >= s.needed))
                .collect();
            if domain.is_empty() {
                return Prune::Missing(Missing {
                    requirements: s.ids.clone(),
                    element: MissingElement::Plane,
                    window: Some(Window::new(Some(s.needed), None)),
                });
            }
            self.seats.push(domain);
        }
        let sigma = self.socket.sigma;
        for c in &self.socket.cones {
            if !self.budget.spend(self.part.cones().len()) {
                return Prune::Exhausted;
            }
            let domain: Vec<usize> = (0..self.part.cones().len())
                .filter(|&k| {
                    self.part.cones().get(k).is_some_and(|x| {
                        x.sense == c.sense
                            && (x.included_angle_rad - c.angle).abs() <= self.wa
                            && x.radii_mm.is_some_and(|[a, b]| {
                                a <= c.radii[0] + sigma + super::signature::slack(c.radii[0])
                                    && b >= c.radii[1] - sigma - super::signature::slack(c.radii[1])
                            })
                    })
                })
                .collect();
            if domain.is_empty() {
                return Prune::Missing(Missing {
                    requirements: vec![c.id.clone()],
                    element: MissingElement::Cone,
                    window: None,
                });
            }
            self.cones.push(domain);
        }
        Prune::Done
    }

    /// The known area of plane `k` with the planes within `W_L` and `W_A`
    /// of it (SL2): one seat may be covered by several coplanar faces. An
    /// offset is compared only across exactly parallel normals; a plane
    /// tilted within `W_A` is counted whatever its offset, which depends on
    /// the frame.
    fn area_near(&self, k: usize) -> f64 {
        let planes = self.part.planes();
        let Some(p) = planes.get(k) else {
            return 0.0;
        };
        let n = p.normal.to_array();
        planes
            .iter()
            .filter(|q| {
                let tilt = angle(n, q.normal.to_array());
                tilt <= self.wa && (tilt > EXACT || (q.offset_mm - p.offset_mm).abs() <= self.wl)
            })
            .map(|q| q.known_area_mm2)
            .sum()
    }

    /// Why no cluster serves `r`: the first test none of its sense passes.
    fn why_no_line(&self, r: &ReqLine) -> Missing {
        let mine: Vec<&Cluster> = self
            .clusters
            .iter()
            .filter(|c| c.sense == r.sense)
            .collect();
        let window = r
            .windows
            .iter()
            .find(|w| !mine.iter().any(|c| c.diameters.iter().any(|d| w.holds(*d))));
        let (element, window) = if let Some(w) = window {
            (MissingElement::Diameter, Some(*w))
        } else if !mine.iter().any(|c| c.openings >= u32::from(r.openings)) {
            (MissingElement::Openings, None)
        } else if mine.is_empty() {
            (MissingElement::Diameter, None)
        } else {
            (
                MissingElement::Extent,
                Some(Window::new(Some(r.to - r.from - self.wl), None)),
            )
        };
        Missing {
            requirements: r.ids.clone(),
            element,
            window,
        }
    }
}

/// SL2: cluster `c` may witness required line `r`.
fn serves(c: &Cluster, r: &ReqLine, wl: f64) -> bool {
    let length = c.hi - c.lo;
    c.sense == r.sense
        && r.windows
            .iter()
            .all(|w| c.diameters.iter().any(|d| w.holds(*d)))
        && c.openings >= u32::from(r.openings)
        && (length.is_nan() || length >= (r.to - r.from) - wl)
}

/// Required elements of a pair: (kind, index).
type Pair = ((Kind, usize), (Kind, usize));

impl Test<'_> {
    /// SL3: the pairs compared, every pair while each kind has at most 64,
    /// else a spanning set.
    fn pairs(&self) -> Vec<Pair> {
        let (nl, ns, nc) = (
            self.socket.lines.len(),
            self.socket.seats.len(),
            self.socket.cones.len(),
        );
        let mut out = Vec::new();
        let mut both = |a: (Kind, usize), b: (Kind, usize)| {
            out.push((a, b));
            out.push((b, a));
        };
        let few = nl <= 64 && ns <= 64;
        for i in 0..nl {
            for j in (i + 1)..nl {
                if few || i == 0 || j == i + 1 {
                    both((Kind::Line, i), (Kind::Line, j));
                }
            }
        }
        for i in 0..ns {
            for j in (i + 1)..ns {
                if few || j == i + 1 {
                    both((Kind::Seat, i), (Kind::Seat, j));
                }
            }
            for j in 0..nl {
                if few || j == 0 {
                    both((Kind::Seat, i), (Kind::Line, j));
                }
            }
        }
        for i in 0..nc {
            for j in 0..nl {
                both((Kind::Cone, i), (Kind::Line, j));
            }
        }
        out
    }

    fn domain(&self, (kind, i): (Kind, usize)) -> &[usize] {
        let d = match kind {
            Kind::Line => self.lines.get(i),
            Kind::Seat => self.seats.get(i),
            Kind::Cone => self.cones.get(i),
        };
        d.map_or(&[], Vec::as_slice)
    }

    fn set_domain(&mut self, (kind, i): (Kind, usize), d: Vec<usize>) {
        let slot = match kind {
            Kind::Line => self.lines.get_mut(i),
            Kind::Seat => self.seats.get_mut(i),
            Kind::Cone => self.cones.get_mut(i),
        };
        if let Some(slot) = slot {
            *slot = d;
        }
    }

    fn ids(&self, (kind, i): (Kind, usize)) -> Vec<RequirementId> {
        match kind {
            Kind::Line => self.socket.lines.get(i).map(|l| l.ids.clone()),
            Kind::Seat => self.socket.seats.get(i).map(|s| s.ids.clone()),
            Kind::Cone => self.socket.cones.get(i).map(|c| vec![c.id.clone()]),
        }
        .unwrap_or_default()
    }

    /// SL4: remove every element with no partner in some pair, until
    /// nothing changes or the budget is spent.
    fn prune(&mut self) -> Prune {
        let pairs = self.pairs();
        let mut changed = true;
        while changed {
            changed = false;
            for &(a, b) in &pairs {
                let (da, db) = (self.domain(a).to_vec(), self.domain(b).to_vec());
                if !self.budget.spend(da.len() * db.len()) {
                    return Prune::Exhausted;
                }
                let kept: Vec<usize> = da
                    .iter()
                    .copied()
                    .filter(|&x| db.iter().any(|&y| self.related(a, b, x, y)))
                    .collect();
                if kept.is_empty() {
                    return Prune::Missing(Missing {
                        requirements: self.ids(a),
                        element: MissingElement::Pair { with: self.ids(b) },
                        window: None,
                    });
                }
                if kept.len() != da.len() {
                    changed = true;
                    self.set_domain(a, kept);
                }
            }
        }
        Prune::Done
    }
}

/// The socket's values are parallel (SG4): decided on its own side, on
/// exact values.
const PARALLEL: f64 = 1e-9;

impl Test<'_> {
    /// SG4: candidate elements `x` (of `a`'s kind) and `y` (of `b`'s) stand
    /// as the requirements `a` and `b` do, within the windows.
    fn related(&self, a: (Kind, usize), b: (Kind, usize), x: usize, y: usize) -> bool {
        match (a.0, b.0) {
            // One witness may serve requirements of one group (SG5).
            (Kind::Line, Kind::Line) => {
                (x != y || self.line_groups.get(a.1) == self.line_groups.get(b.1))
                    && self.lines_related(a.1, b.1, x, y)
            }
            (Kind::Seat, Kind::Seat) => {
                (x != y || self.seat_groups.get(a.1) == self.seat_groups.get(b.1))
                    && self.seats_related(a.1, b.1, x, y)
            }
            (Kind::Seat, Kind::Line) => self.seat_line(a.1, b.1, x, y),
            (Kind::Line, Kind::Seat) => self.seat_line(b.1, a.1, y, x),
            (Kind::Cone, Kind::Line) => self.cone_line(a.1, b.1, x, y),
            (Kind::Line, Kind::Cone) => self.cone_line(b.1, a.1, y, x),
            _ => true,
        }
    }

    fn lines_related(&self, a: usize, b: usize, x: usize, y: usize) -> bool {
        let (Some(ra), Some(rb), Some(ca), Some(cb)) = (
            self.socket.lines.get(a),
            self.socket.lines.get(b),
            self.clusters.get(x),
            self.clusters.get(y),
        ) else {
            return false;
        };
        let required = line_angle(ra.dir, rb.dir);
        if (required - line_angle(ca.dir, cb.dir)).abs() > self.wa {
            return false;
        }
        required > PARALLEL
            || (point_line(rb.mid(), ra.point, ra.dir) - point_line(cb.mid(), ca.point, ca.dir))
                .abs()
                <= self.wl
    }

    fn seats_related(&self, a: usize, b: usize, x: usize, y: usize) -> bool {
        let planes = self.part.planes();
        let (Some(ra), Some(rb), Some(ca), Some(cb)) = (
            self.socket.seats.get(a),
            self.socket.seats.get(b),
            planes.get(x),
            planes.get(y),
        ) else {
            return false;
        };
        let (na, nb) = (ca.normal.to_array(), cb.normal.to_array());
        let required = angle(ra.normal, rb.normal);
        let found = angle(na, nb);
        if (required - found).abs() > self.wa {
            return false;
        }
        // Offsets compare only between exactly parallel planes: across a
        // tilt they depend on where the part sits in its frame.
        let exact = found <= EXACT || found >= std::f64::consts::PI - EXACT;
        if !exact {
            true
        } else if required <= PARALLEL {
            ((rb.offset - ra.offset) - (cb.offset_mm - ca.offset_mm)).abs() <= self.wl
        } else if required >= std::f64::consts::PI - PARALLEL {
            ((rb.offset + ra.offset) - (cb.offset_mm + ca.offset_mm)).abs() <= self.wl
        } else {
            true
        }
    }

    fn seat_line(&self, seat: usize, line: usize, plane: usize, cluster: usize) -> bool {
        let (Some(rs), Some(rl), Some(plane), Some(cluster)) = (
            self.socket.seats.get(seat),
            self.socket.lines.get(line),
            self.part.planes().get(plane),
            self.clusters.get(cluster),
        ) else {
            return false;
        };
        let normal = plane.normal.to_array();
        if (line_angle(rs.normal, rl.dir) - line_angle(normal, cluster.dir)).abs() > self.wa {
            return false;
        }
        let required = dot(rs.normal, rl.mid()) - rs.offset;
        let found = dot(normal, cluster.mid()) - plane.offset_mm;
        dot(rs.normal, rl.dir).abs() > PARALLEL || (required - found).abs() <= self.wl
    }

    fn cone_line(&self, cone: usize, line: usize, candidate: usize, cluster: usize) -> bool {
        let (Some(rc), Some(rl), Some(candidate), Some(cluster)) = (
            self.socket.cones.get(cone),
            self.socket.lines.get(line),
            self.part.cones().get(candidate),
            self.clusters.get(cluster),
        ) else {
            return false;
        };
        let coaxial =
            line_angle(rc.dir, rl.dir) <= PARALLEL && point_line(rc.apex, rl.point, rl.dir) <= 1e-6;
        !coaxial
            || (line_angle(candidate.axis.direction.to_array(), cluster.dir) <= self.wa
                && point_line(candidate.apex_mm, cluster.point, cluster.dir) <= self.wl)
    }

    /// SL5: distinct groups of required lines need distinct witness
    /// stacks, distinct groups of seats distinct planes; a group's witness
    /// is any of its members'.
    fn distinct(&mut self) -> Option<Missing> {
        for (kind, domains, groups) in [
            (Kind::Line, self.lines.clone(), self.line_groups.clone()),
            (Kind::Seat, self.seats.clone(), self.seat_groups.clone()),
        ] {
            let n = match kind {
                Kind::Line => self.clusters.len(),
                _ => self.part.planes().len(),
            };
            // Each group, by its least member, with its members' domains.
            let mut united: std::collections::BTreeMap<usize, (usize, Vec<usize>)> =
                std::collections::BTreeMap::new();
            for (k, d) in domains.iter().enumerate() {
                let g = groups.get(k).copied().unwrap_or(k);
                let entry = united.entry(g).or_insert((k, Vec::new()));
                entry.1.extend(d.iter().copied());
            }
            let (members, mut sets): (Vec<usize>, Vec<Vec<usize>>) = united.into_values().unzip();
            for set in &mut sets {
                set.sort_unstable();
                set.dedup();
            }
            if let Some(i) = matching(&sets, n, &mut self.budget) {
                let first = members.get(i).copied().unwrap_or(0);
                return Some(Missing {
                    requirements: self.ids((kind, first)),
                    element: MissingElement::Distinct,
                    window: None,
                });
            }
        }
        None
    }
}

/// A matching of each domain to a distinct element (augmenting paths);
/// the first that cannot be matched, or `None`. A spent budget matches
/// nothing more and finds no fault.
fn matching(domains: &[Vec<usize>], n: usize, budget: &mut Budget) -> Option<usize> {
    let mut owner: Vec<Option<usize>> = vec![None; n];
    for u in 0..domains.len() {
        let mut seen = vec![false; n];
        match augment(u, domains, &mut owner, &mut seen, (budget, 0)) {
            Some(true) => {}
            Some(false) => return Some(u),
            None => return None,
        }
    }
    None
}

/// How deep an augmenting search may go before it counts as a spent
/// budget: a socket of thousands of lines keeps the candidate rather than
/// overflow the stack.
const MAX_DEPTH: usize = 4_096;

/// One augmenting search from `u`; `None` once the budget is spent.
fn augment(
    u: usize,
    domains: &[Vec<usize>],
    owner: &mut [Option<usize>],
    seen: &mut [bool],
    (budget, depth): (&mut Budget, usize),
) -> Option<bool> {
    if depth > MAX_DEPTH {
        budget.0 = MAX_SHORTLIST_CHECKS.saturating_add(1);
        return None;
    }
    for &v in domains.get(u).map_or(&[][..], Vec::as_slice) {
        if !budget.spend(1) {
            return None;
        }
        if seen.get(v).copied().unwrap_or(true) {
            continue;
        }
        if let Some(s) = seen.get_mut(v) {
            *s = true;
        }
        let free = match owner.get(v).copied().flatten() {
            None => true,
            Some(w) => augment(w, domains, owner, seen, (&mut *budget, depth + 1))?,
        };
        if free {
            if let Some(o) = owner.get_mut(v) {
                *o = Some(u);
            }
            return Some(true);
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::{Budget, matching};

    /// SL5: distinct requirements need distinct witnesses; augmenting
    /// paths find a matching where one exists.
    #[test]
    fn distinct_requirements_need_distinct_witnesses() {
        let mut budget = Budget(0);
        assert_eq!(matching(&[vec![0], vec![0]], 2, &mut budget), Some(1));
        assert_eq!(matching(&[vec![0, 1], vec![0]], 2, &mut budget), None);
        assert_eq!(
            matching(&[vec![0, 1], vec![0, 1], vec![1]], 2, &mut budget),
            Some(2)
        );
    }
}
