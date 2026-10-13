//! The relations between one target occurrence and every neighbour
//! (`docs/research/m3-socket.md`, section 7): B, I and J between axial
//! elements, B8's unread occupants, J3's blocks, and the unknowns U4, U5
//! and U7.
// a, b, p, t, n, e and the like are the design's own names for targets,
// neighbours, pairs and stations.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use super::axial::{self, Element, EndKind, Pair, Side};
use super::block;
use super::geo::{Motion, V, add, dot, norm, perpendiculars, scale, sub};
use super::model::{Fit, Flag};
use super::place::{Aabb, Tree};
use super::{CONTACT_MM, MAX_RELATIONS, NEAR_MM, PARALLEL_SINE_REL};
use crate::features::extent::FaceBox;
use crate::features::{Extent, FeatureId, Part};

/// A neighbour occurrence, moved into the target's frame.
pub(crate) struct Neighbour<'a> {
    /// Its path, by usage `#id`s.
    pub(crate) path: Vec<u64>,
    pub(crate) part: &'a Part,
    /// Its faces' boxes near the target (N4).
    pub(crate) faces: Vec<FaceBox>,
    /// From its frame into the target's.
    pub(crate) motion: Motion,
    pub(crate) elements: Vec<Element>,
    /// The largest of its uncertainties and the target occurrence's (Z2).
    pub(crate) floor: f64,
    /// Another occurrence of the target (N2).
    pub(crate) selected: bool,
    /// Its extent, in the target's frame, may come within reach of the
    /// target's (N3); always true when either has none.
    pub(crate) near: bool,
    /// Its definition's index (N4).
    pub(crate) index: Option<&'a super::index::Index>,
    /// The target's box grown by reach, in its frame: what of it is read
    /// (N4); `None` for all of it.
    pub(crate) region: Option<Aabb>,
}

/// One relation, before it is combined into a requirement.
#[derive(Debug, Clone)]
pub(crate) struct Relation {
    /// The target element it concerns.
    pub(crate) target: usize,
    pub(crate) rule: &'static str,
    /// The neighbour, an index into the neighbours given.
    pub(crate) neighbour: usize,
    pub(crate) feature: Option<FeatureId>,
    pub(crate) faces: Vec<u64>,
    pub(crate) e_max: Option<f64>,
    pub(crate) sine: Option<f64>,
    pub(crate) margin: Option<f64>,
    pub(crate) gap: Option<f64>,
    /// The stations it covers on the target element's axis.
    pub(crate) span: Option<(f64, f64)>,
    /// What it asks there; `None` for provenance only.
    pub(crate) fit: Option<Fit>,
    /// The clear diameter (B) or the largest allowed (I).
    pub(crate) clear: Option<f64>,
    /// J's pin: only where no B relation occupies the hole.
    pub(crate) unoccupied_only: bool,
    pub(crate) open_start: bool,
    pub(crate) open_finish: bool,
    /// How far a B relation's shaft runs past the target element's start
    /// and finish; negative short of them.
    pub(crate) beyond: (f64, f64),
    /// I4: how much longer a target shaft may be before its free end meets
    /// the floor of the blind neighbour hole holding it.
    pub(crate) extra: Option<f64>,
    pub(crate) flags: BTreeSet<Flag>,
    pub(crate) floor: f64,
}

impl Relation {
    fn new(target: usize, rule: &'static str, neighbour: usize, floor: f64) -> Self {
        Self {
            target,
            rule,
            neighbour,
            feature: None,
            faces: Vec::new(),
            e_max: None,
            sine: None,
            margin: None,
            gap: None,
            span: None,
            fit: None,
            clear: None,
            unoccupied_only: false,
            open_start: false,
            open_finish: false,
            beyond: (f64::NEG_INFINITY, f64::NEG_INFINITY),
            extra: None,
            flags: BTreeSet::new(),
            floor,
        }
    }
}

/// What the rules found for one target occurrence.
#[derive(Debug, Default)]
pub(crate) struct Found {
    pub(crate) relations: Vec<Relation>,
    /// Target features neighbour material crosses at an angle (U7), with
    /// the faces.
    pub(crate) skew: BTreeMap<FeatureId, BTreeSet<u64>>,
    /// Target holes whose only partner beyond a gap is blocked (J3).
    pub(crate) blocked: BTreeSet<FeatureId>,
    /// H1: a wider neighbour cylinder ending at a target hole's open rim.
    pub(crate) rings: Vec<Ring>,
}

/// A wider neighbour cylinder ending at a target hole's open rim (H1).
#[derive(Debug, Clone)]
pub(crate) struct Ring {
    /// The target plane the rim lies on.
    pub(crate) plane: FeatureId,
    /// The target hole.
    pub(crate) hole: FeatureId,
    pub(crate) neighbour: usize,
    pub(crate) faces: Vec<u64>,
    pub(crate) inner: f64,
    pub(crate) outer: f64,
    pub(crate) floor: f64,
}

/// Check 39's count was spent.
#[derive(Debug)]
pub(crate) struct TooMany;

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

/// The fit of a margin (B1 to B4, I1, I2).
fn fit_of(margin: f64, e_max: f64, tau: f64, flags: &mut BTreeSet<Flag>) -> Fit {
    if margin > tau {
        if e_max > tau {
            flags.insert(Flag::Offset);
        }
        Fit::Clearance
    } else if margin >= -tau {
        flags.insert(Flag::ThreadNotConfirmed);
        Fit::LineToLine
    } else if e_max <= tau {
        flags.insert(Flag::ThreadNotConfirmed);
        Fit::Interference
    } else {
        flags.insert(Flag::Conflict);
        Fit::Interference
    }
}

/// The span a relation covers on `a`, a touch giving one station.
fn span(a: &Element, p: &Pair, tau: f64) -> (f64, f64) {
    if p.to >= p.from {
        (p.from, p.to)
    } else if p.from <= a.lo + tau {
        (a.lo, a.lo)
    } else {
        (a.hi, a.hi)
    }
}

/// True if the relation exists only through a side whose extent is not
/// known (U3).
fn through_unknown(a: &Element, b: &Element, p: &Pair) -> bool {
    let along = dot(a.dir, b.dir) > 0.0;
    let (lo_unknown, hi_unknown) = if along {
        (b.lo_unknown, b.hi_unknown)
    } else {
        (b.hi_unknown, b.lo_unknown)
    };
    b.undecided
        || a.undecided
        || !p.e_max.is_finite()
        || (p.lo > a.lo && lo_unknown)
        || (p.hi < a.hi && hi_unknown)
}

/// A neighbour hole beyond a rim: (gap, neighbour, element, pair).
type Beyond = (f64, usize, usize, Pair);

struct Search<'s, 'a> {
    targets: &'s [Element],
    neighbours: &'s [Neighbour<'a>],
    /// Check 39's count, across every occurrence of the extraction.
    pairs: &'s mut u64,
    /// The neighbour features each (target element, neighbour) B relation
    /// reads, for B8.
    read: BTreeMap<(usize, usize), BTreeSet<FeatureId>>,
    found: Found,
    /// The holes beyond each open rim: (target element, start?) to
    /// (gap, neighbour, element, pair).
    beyond: BTreeMap<(usize, bool), Vec<Beyond>>,
    /// The target elements' boxes (N4).
    tree: Tree,
}

/// Every relation between the target's `targets` (in its own frame) and
/// `neighbours`, and the unknowns; `TooMany` once `pairs`, check 39's
/// count, passes its limit.
pub(crate) fn relate(
    targets: &[Element],
    neighbours: &[Neighbour<'_>],
    planes: &[(Motion, &Part)],
    pairs: &mut u64,
) -> Result<Found, TooMany> {
    let mut search = Search {
        targets,
        neighbours,
        pairs,
        read: BTreeMap::new(),
        found: Found::default(),
        beyond: BTreeMap::new(),
        tree: Tree::new(targets.iter().map(element_box).collect()),
    };
    // N4: each neighbour element against the target elements it may
    // relate to: those its box comes within reach of, and for a hole those
    // its axis passes near at any distance (J). In the order of a full
    // scan, target by target.
    let widest = targets.iter().map(|a| a.radius).fold(0.0, f64::max);
    for (n, neighbour) in neighbours.iter().enumerate() {
        let by = reach(neighbour.floor);
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (e, b) in neighbour.elements.iter().enumerate() {
            let near = element_box(b).map_or_else(
                || (0..targets.len()).collect(),
                |x| search.tree.meeting(&x, by),
            );
            pairs.extend(near.into_iter().map(|t| (t, e)));
            if b.side == Side::Hole {
                let line = search.tree.along(b.point, b.dir, b.radius + widest + by);
                pairs.extend(line.into_iter().map(|t| (t, e)));
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        for (t, e) in pairs {
            let (Some(a), Some(b)) = (targets.get(t), neighbour.elements.get(e)) else {
                continue;
            };
            search.count()?;
            search.axial(t, a, n, e, b);
        }
    }
    search.continuations(planes)?;
    for r in &search.found.relations {
        if let (true, Some(f)) = (r.rule.starts_with('B'), r.feature) {
            search
                .read
                .entry((r.target, r.neighbour))
                .or_default()
                .insert(f);
        }
    }
    for (n, neighbour) in neighbours.iter().enumerate() {
        if neighbour.near {
            search.occupants(n)?;
        }
    }
    Ok(search.found)
}

impl Search<'_, '_> {
    fn count(&mut self) -> Result<(), TooMany> {
        *self.pairs += 1;
        if *self.pairs > MAX_RELATIONS {
            Err(TooMany)
        } else {
            Ok(())
        }
    }

    fn axial(&mut self, t: usize, a: &Element, n: usize, e: usize, b: &Element) {
        let Some(neighbour) = self.neighbours.get(n) else {
            return;
        };
        let tau = CONTACT_MM.max(neighbour.floor);
        match (a.side, b.side) {
            // Partial cylinders: a target one takes what occupies it or
            // holds it (U1); a neighbour's partial hole holds a target
            // shaft (I7); a neighbour's partial shaft in a target hole is
            // B8's, as unread material.
            (Side::Hole, Side::Shaft) if b.partial.is_some() => self.skew_only(a, b, tau),
            (Side::Hole, Side::Hole) | (Side::Shaft, Side::Shaft)
                if a.partial.is_some() || b.partial.is_some() => {}
            (Side::Hole, Side::Hole) => self.continuation(t, a, n, e, b, tau),
            (_, Side::Shaft) if !neighbour.near => {}
            (Side::Hole, Side::Shaft) => self.occupied(t, a, n, b, tau),
            (Side::Shaft, Side::Hole) if neighbour.near => self.held(t, a, n, b, tau),
            (Side::Shaft, Side::Shaft) => self.abutting(t, a, n, b, tau),
            (Side::Shaft, Side::Hole) => {}
        }
    }

    /// U7 alone, for neighbour material no axial rule reads.
    fn skew_only(&mut self, a: &Element, b: &Element, tau: f64) {
        if axial::pair(a, b, PARALLEL_SINE_REL, 1.0).is_none() {
            self.skew(a, b, tau);
        }
    }

    /// U7: neighbour material at an angle whose cross-section may meet the
    /// target's: a shaft, or the wall of a hole a target shaft runs through.
    fn skew(&mut self, a: &Element, b: &Element, tau: f64) {
        let material = b.side == Side::Shaft || a.side == Side::Shaft;
        if material && axial::segment_distance(a, b) < a.radius + b.radius + tau {
            self.found
                .skew
                .entry(a.feature)
                .or_default()
                .extend(b.wall.iter().copied());
        }
    }

    fn relation(&self, t: usize, rule: &'static str, n: usize, b: &Element, p: &Pair) -> Relation {
        let floor = self.neighbours.get(n).map_or(0.0, |x| x.floor);
        let mut relation = Relation::new(t, rule, n, floor);
        relation.feature = Some(b.feature);
        relation.faces.clone_from(&b.wall);
        relation.e_max = finite(p.e_max);
        relation.sine = Some(p.sine);
        if self.neighbours.get(n).is_some_and(|x| x.selected) {
            relation.flags.insert(Flag::SelfPaired);
        }
        relation
    }

    /// B1 to B7: a neighbour shaft in a target hole.
    fn occupied(&mut self, t: usize, a: &Element, n: usize, b: &Element, tau: f64) {
        let Some(p) = axial::pair(a, b, PARALLEL_SINE_REL, 1.0) else {
            self.skew(a, b, tau);
            return;
        };
        if p.e_min >= a.radius + b.radius / p.cosine + tau || p.length <= -tau {
            return;
        }
        let reach = p.e_max + b.radius / p.cosine;
        let margin = a.radius - reach;
        // A ring around the hole's part, clear of it: a bearing round a
        // spacer, seen from the spacer.
        if margin < -tau {
            if let (Some(neighbour), Some(q)) = (
                self.neighbours.get(n),
                axial::pair(b, a, PARALLEL_SINE_REL, 0.0),
            ) {
                if hollow(&neighbour.elements, b, self.targets, &q, tau) {
                    // Read, and clear of the hole: not for B8.
                    self.read.entry((t, n)).or_default().insert(b.feature);
                    return;
                }
            }
        }
        // A wider cylinder that only touches the rim does not enter: a head
        // bearing there (H1), not an occupant.
        if p.length.abs() <= tau && margin < -tau {
            // Only a known end bears (U3): a flange whose end the rim meets
            // only past an unread side (a valve's far flange) is no ring.
            if let Some(q) = axial::pair(a, &known(b), PARALLEL_SINE_REL, 1.0) {
                if q.length.abs() <= tau {
                    self.ring(a, n, b, &q, tau);
                }
            }
            return;
        }
        let mut relation = self.relation(t, "B", n, b, &p);
        let fit = fit_of(margin, p.e_max, tau, &mut relation.flags);
        relation.rule = match fit {
            Fit::Clearance => "B1",
            Fit::LineToLine => "B2",
            _ if relation.flags.contains(&Flag::Conflict) => "B4",
            _ => "B3",
        };
        relation.margin = finite(margin);
        relation.fit = Some(fit);
        relation.clear = finite(2.0 * reach);
        relation.span = Some(span(a, &p, tau));
        if through_unknown(a, b, &p) {
            relation.flags.insert(Flag::ExtentUnknown);
        }
        // B6: the shaft continues past a rim.
        let last = a.segment + 1 == a.segments;
        relation.open_start = a.segment == 0 && p.lo < a.lo - tau;
        relation.open_finish = last && p.hi > a.hi + tau;
        if p.length > tau {
            relation.beyond = (a.lo - p.lo, p.hi - a.hi);
        }
        for (open, end) in [
            (relation.open_start, a.start),
            (relation.open_finish, a.finish),
        ] {
            match (open, end) {
                (true, EndKind::Closed) => {
                    relation.flags.insert(Flag::Conflict);
                }
                (true, EndKind::Unknown) => {
                    relation.flags.insert(Flag::OpenAtUnknownEnd);
                }
                _ => {}
            }
        }
        self.push_split(&relation, a, b, &p, 1.0, tau);
    }

    /// H1: a wider neighbour cylinder, near enough the axis to cover the
    /// rim all round, touching an open rim of a target hole: the plane the
    /// hole opens onto there is a seat.
    fn ring(&mut self, a: &Element, n: usize, b: &Element, p: &Pair, tau: f64) {
        // H2: a cylinder whose axis misses the hole (a flange over bolt
        // holes off its axis) is C1's, which reads its end face.
        if p.e_max > b.radius - a.radius
            || p.e_max >= a.radius
            || a.partial.is_some()
            || a.undecided
        {
            return;
        }
        // The rim it touches: an open end, or a step inside the stack.
        let at_start = (p.hi - a.lo).abs() <= tau || (p.lo - a.lo).abs() <= tau;
        let plane = if at_start {
            a.start_into
        } else {
            a.finish_into
        };
        let Some(plane) = plane else {
            return;
        };
        self.found.rings.push(Ring {
            plane,
            hole: a.feature,
            neighbour: n,
            faces: b.wall.clone(),
            inner: 2.0 * a.radius,
            outer: 2.0 * b.radius,
            floor: self.neighbours.get(n).map_or(0.0, |x| x.floor),
        });
    }

    /// I1 to I3: a target shaft in a neighbour hole.
    fn held(&mut self, t: usize, a: &Element, n: usize, b: &Element, tau: f64) {
        let Some(p) = axial::pair(a, b, PARALLEL_SINE_REL, -1.0) else {
            self.skew(a, b, tau);
            return;
        };
        if p.e_min >= a.radius + b.radius / p.cosine + tau || p.length <= -tau {
            return;
        }
        let room = b.radius - p.e_max;
        let margin = room - a.radius;
        // I7: a neighbour's partial hole (a slot end, a V) holds the
        // target shaft: pinned, since its sector is not read.
        if b.partial.is_some() {
            let mut relation = self.relation(t, "I7", n, b, &p);
            relation.fit = Some(Fit::NotRead);
            relation.span = Some(span(a, &p, tau));
            relation.flags.insert(Flag::PartialNeighbour);
            self.found.relations.push(relation);
            return;
        }
        // A ring around the hole's part, clear of it: a bearing round a
        // spacer, seen from the bearing.
        if margin < -tau {
            if let Some(neighbour) = self.neighbours.get(n) {
                if hollow(self.targets, a, &neighbour.elements, &p, tau) {
                    return;
                }
            }
        }
        // A shaft wider than the hole that only touches its rim does not
        // enter it: its end bears there.
        if p.length.abs() <= tau && margin < -tau {
            return;
        }
        let mut relation = self.relation(t, "I", n, b, &p);
        let fit = fit_of(margin, p.e_max, tau, &mut relation.flags);
        relation.rule = if fit == Fit::Clearance { "I1" } else { "I2" };
        relation.margin = finite(margin);
        relation.fit = Some(fit);
        relation.clear = finite(2.0 * room).filter(|_| fit == Fit::Clearance);
        relation.span = Some(span(a, &p, tau));
        if through_unknown(a, b, &p) {
            relation.flags.insert(Flag::ExtentUnknown);
        }
        let stack = self
            .neighbours
            .get(n)
            .map_or(&[][..], |x| x.elements.as_slice());
        relation.extra = to_floor(a, b, &p, tau, stack);
        self.push_split(&relation, a, b, &p, -1.0, tau);
    }

    /// U3 and X7: `relation`'s fit holds where both elements are known,
    /// and a line-to-line or interference fit only where both cylinders
    /// themselves are; past a known end, where an unknown side was run to
    /// the part's extent, nothing is read of either: fit `not_read`,
    /// flagged `extent_unknown` (a countersunk head past a shank's unknown
    /// end, a flange past a bore's).
    fn push_split(
        &mut self,
        relation: &Relation,
        a: &Element,
        b: &Element,
        p: &Pair,
        grow: f64,
        tau: f64,
    ) {
        let known = axial::pair(&known(a), &known(b), PARALLEL_SINE_REL, grow)
            .filter(|q| q.length > -tau)
            .map(|q| (q.from, q.to));
        let mut rest = Vec::new();
        match known {
            Some((from, to)) => {
                if from - p.from > tau {
                    rest.push((p.from, from));
                }
                if p.to - to > tau {
                    rest.push((to, p.to));
                }
            }
            None if p.length > tau => rest.push((p.from, p.to)),
            None => {}
        }
        let mut carried = false;
        if let Some((from, to)) = known {
            let mut main = relation.clone();
            if !rest.is_empty() {
                main.span = main.span.map(|(x, y)| (x.max(from), y.min(to)));
            }
            let fits = main.fit == Some(Fit::Clearance) || main.fit.is_none();
            if fits || own_overlap(&mut main, a, b, grow, tau) {
                self.found.relations.push(main);
                carried = true;
            }
        }
        for (x, y) in rest {
            let mut unread = relation.clone();
            unread.rule = "U3";
            unread.fit = Some(Fit::NotRead);
            unread.clear = None;
            unread.margin = None;
            unread.span = Some((x, y));
            unread.flags.remove(&Flag::ThreadNotConfirmed);
            unread.flags.insert(Flag::ExtentUnknown);
            if carried {
                unread.open_start = false;
                unread.open_finish = false;
                unread.beyond = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            }
            carried = true;
            self.found.relations.push(unread);
        }
    }

    /// I5, I6: coaxial shafts; only those end to end ask anything.
    fn abutting(&mut self, t: usize, a: &Element, n: usize, b: &Element, tau: f64) {
        let Some(p) = axial::pair(a, b, PARALLEL_SINE_REL, 0.0) else {
            self.skew(a, b, tau);
            return;
        };
        if p.e_max <= tau && p.length.abs() <= tau {
            let mut relation = self.relation(t, "I6", n, b, &p);
            relation.flags.insert(Flag::Abutting);
            self.found.relations.push(relation);
        }
    }

    /// J1, J2 candidates: the neighbour holes beyond each open rim.
    fn continuation(&mut self, t: usize, a: &Element, n: usize, e: usize, b: &Element, tau: f64) {
        let Some(p) = axial::pair(a, b, PARALLEL_SINE_REL, 0.0) else {
            return;
        };
        if p.e_min >= a.radius + b.radius / p.cosine + tau {
            return;
        }
        let rims = [
            (
                true,
                a.segment == 0 && a.start == EndKind::Open,
                a.start_rim,
            ),
            (
                false,
                a.segment + 1 == a.segments && a.finish == EndKind::Open,
                a.finish_rim,
            ),
        ];
        for (start, applies, rim) in rims {
            let (true, Some(rim)) = (applies, rim) else {
                continue;
            };
            let gap = if start {
                (p.hi <= rim + tau).then_some(rim - p.hi)
            } else {
                (p.lo >= rim - tau).then_some(p.lo - rim)
            };
            let Some(gap) = gap.filter(|g| g.is_finite()) else {
                continue;
            };
            self.beyond
                .entry((t, start))
                .or_default()
                .push((gap, n, e, p));
        }
    }

    /// J1 to J3: the nearest hole beyond each rim that nothing blocks. A
    /// blocked hole does not block a farther one: J3's disc is as narrow
    /// as the narrower hole, and a farther one may be wider.
    fn continuations(&mut self, planes: &[(Motion, &Part)]) -> Result<(), TooMany> {
        let beyond = std::mem::take(&mut self.beyond);
        for ((t, start), mut candidates) in beyond {
            candidates.sort_by(|x, y| x.0.total_cmp(&y.0).then((x.1, x.2).cmp(&(y.1, y.2))));
            let mut blocked = false;
            for (gap, n, e, p) in candidates {
                blocked = self.continuation_to(t, start, gap, n, e, &p, planes)?;
                if !blocked {
                    break;
                }
            }
            if blocked {
                if let Some(a) = self.targets.get(t) {
                    self.found.blocked.insert(a.feature);
                }
            }
        }
        Ok(())
    }

    /// J1 or J2 from target element `t`'s rim to neighbour `n`'s element
    /// `e`, unless J3 blocks it (`true`). J's pin is provenance only when a
    /// B relation's shaft runs from inside the target hole past the rim
    /// and into this hole (J1's "occupies both").
    #[allow(clippy::too_many_arguments)]
    fn continuation_to(
        &mut self,
        t: usize,
        start: bool,
        gap: f64,
        n: usize,
        e: usize,
        p: &Pair,
        planes: &[(Motion, &Part)],
    ) -> Result<bool, TooMany> {
        let (Some(a), Some(neighbour)) = (self.targets.get(t), self.neighbours.get(n)) else {
            return Ok(false);
        };
        let Some(b) = neighbour.elements.get(e) else {
            return Ok(false);
        };
        let tau = CONTACT_MM.max(neighbour.floor);
        let rim = if start { a.start_rim } else { a.finish_rim }.unwrap_or(0.0);
        if gap > tau {
            let (from, to) = if start {
                (rim - gap, rim)
            } else {
                (rim, rim + gap)
            };
            let radius = a.radius.min(b.radius) + p.e_max + tau;
            if self.blocked(a, from, to, radius, tau, planes)? {
                return Ok(true);
            }
        }
        let spanned = self.found.relations.iter().any(|r| {
            r.target == t
                && r.rule.starts_with('B')
                && r.rule != "B8"
                && if start { r.beyond.0 } else { r.beyond.1 } > gap + tau
        });
        let mut relation = self.relation(t, if gap > tau { "J2" } else { "J1" }, n, b, p);
        relation.gap = finite(gap);
        if !spanned {
            relation.fit = Some(Fit::NotRead);
            relation.unoccupied_only = true;
        }
        relation.open_start = start;
        relation.open_finish = !start;
        self.found.relations.push(relation);
        Ok(false)
    }

    /// J3: a plane face with a known outline lies across `a`'s axis
    /// strictly between `from` and `to`, with a disc of `radius` about the
    /// crossing inside its outline (grown for a tilted face).
    fn blocked(
        &mut self,
        a: &Element,
        from: f64,
        to: f64,
        radius: f64,
        tau: f64,
        planes: &[(Motion, &Part)],
    ) -> Result<bool, TooMany> {
        for (motion, part) in planes {
            let back = motion.inverse();
            for plane in &part.planes {
                let normal = motion.dir(plane.normal.to_array());
                let cosine = dot(normal, a.dir).abs();
                if cosine < 1e-9 {
                    continue;
                }
                for face in &plane.faces {
                    self.count()?;
                    let anchor = motion.point(scale(plane.normal.to_array(), face.offset_mm));
                    let s = dot(sub(anchor, a.point), normal) / dot(a.dir, normal);
                    if !(s > from + tau && s < to - tau) {
                        continue;
                    }
                    let crossing = back.point(add(a.point, scale(a.dir, s)));
                    if block::disc_inside(
                        &face.outline,
                        crossing,
                        plane.normal.to_array(),
                        radius / cosine + tau,
                    ) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// B8: neighbour faces that may lie in a target hole's open core and
    /// that no B relation of the same neighbour reads.
    fn occupants(&mut self, n: usize) -> Result<(), TooMany> {
        let Some(neighbour) = self.neighbours.get(n) else {
            return Ok(());
        };
        let tau = CONTACT_MM.max(neighbour.floor);
        let shapes = super::touch::Shapes::of(neighbour);
        let round = round_faces(neighbour);
        // N4: only the holes a face of the neighbour may come near; a face
        // with no box may come near any.
        let mut holes: Vec<usize> = Vec::new();
        for face in &neighbour.faces {
            match face.bounds.map(|b| moved(b, &neighbour.motion)) {
                Some(bounds) => holes.extend(self.tree.meeting(&bounds, tau)),
                None => holes.extend(0..self.targets.len()),
            }
        }
        holes.sort_unstable();
        holes.dedup();
        for t in holes {
            let Some(a) = self.targets.get(t) else {
                continue;
            };
            if a.side != Side::Hole
                || a.partial.is_some()
                || a.hi - a.lo <= 2.0 * tau
                || a.radius <= tau
            {
                continue;
            }
            let features = self.read.get(&(t, n));
            let read: BTreeSet<u64> = neighbour
                .elements
                .iter()
                .filter(|e| features.is_some_and(|f| f.contains(&e.feature)))
                .flat_map(|e| e.faces.iter().copied())
                .collect();
            let mut faces = Vec::new();
            let mut reach = (f64::INFINITY, f64::NEG_INFINITY);
            for face in &neighbour.faces {
                self.count()?;
                if read.contains(&face.face) {
                    continue;
                }
                // A cylinder parallel to the hole that stays outside its
                // core (a wider coaxial bore beside it) meets nothing in it,
                // whatever its box says.
                if let Some((p, d, radius)) = round.get(&face.face) {
                    let e = norm(super::geo::across(sub(*p, a.point), a.dir));
                    if norm(super::geo::cross(*d, a.dir)) <= 1e-9 && radius - e >= a.radius - tau {
                        continue;
                    }
                }
                let meets = match shapes.meets_core(face.face, a, tau) {
                    Some(super::touch::Core::Apart) => None,
                    Some(super::touch::Core::Meets(stations)) => Some(stations),
                    None => meets_core(a, face, &neighbour.motion, tau),
                };
                if let Some((lo, hi)) = meets {
                    faces.push(face.face);
                    reach = (reach.0.min(lo), reach.1.max(hi));
                }
            }
            if faces.is_empty() {
                continue;
            }
            faces.sort_unstable();
            let mut relation = Relation::new(t, "B8", n, neighbour.floor);
            relation.faces = faces;
            relation.fit = Some(Fit::NotRead);
            relation.span = Some((a.own_lo, a.own_hi));
            relation.flags.insert(Flag::OccupantUnread);
            // Material that runs on past a rim needs that end open. A face
            // with no box says nothing of where it is.
            relation.open_start = a.segment == 0 && reach.0.is_finite() && reach.0 < a.lo - tau;
            relation.open_finish =
                a.segment + 1 == a.segments && reach.1.is_finite() && reach.1 > a.hi + tau;
            for (open, end) in [
                (relation.open_start, a.start),
                (relation.open_finish, a.finish),
            ] {
                match (open, end) {
                    (true, EndKind::Closed) => {
                        relation.flags.insert(Flag::Conflict);
                    }
                    (true, EndKind::Unknown) => {
                        relation.flags.insert(Flag::OpenAtUnknownEnd);
                    }
                    _ => {}
                }
            }
            if neighbour.selected {
                relation.flags.insert(Flag::SelfPaired);
            }
            self.found.relations.push(relation);
        }
        Ok(())
    }
}

/// A neighbour's hole-sense cylinder faces near the target, by face: axis
/// point, direction and radius, in the target's frame. Only air lies
/// inside one; a shaft's cylinder, even a D-shaft's partial one, is
/// material.
fn round_faces(neighbour: &Neighbour<'_>) -> BTreeMap<u64, (V, V, f64)> {
    let kept: BTreeSet<u64> = neighbour.faces.iter().map(|b| b.face).collect();
    neighbour
        .faces
        .iter()
        .filter_map(|b| match neighbour.index.and_then(|i| i.at.get(&b.face)) {
            Some(super::index::FaceAt::Cylinder(k)) => neighbour.part.cylinders.get(*k),
            _ => None,
        })
        .chain(
            neighbour
                .index
                .is_none()
                .then_some(&neighbour.part.cylinders)
                .into_iter()
                .flatten(),
        )
        .filter(|c| c.sense == crate::features::Sense::Hole)
        .flat_map(|c| {
            let axis = (
                neighbour.motion.point(c.axis.point_mm.to_array()),
                neighbour.motion.dir(c.axis.direction.to_array()),
                c.radius_mm,
            );
            c.faces.iter().map(move |f| (*f, axis))
        })
        .filter(|(f, _)| kept.contains(f))
        .collect()
}

/// A box moved by `motion`: the box of its moved corners.
fn moved(bounds: ([f64; 3], [f64; 3]), motion: &Motion) -> Aabb {
    let (low, high) = bounds;
    let mut out = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for i in 0..8 {
        let p = motion.point([
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ]);
        for ((low, high), x) in out.0.iter_mut().zip(out.1.iter_mut()).zip(p) {
            *low = low.min(x);
            *high = high.max(x);
        }
    }
    out
}

/// An element's box: its axis segment grown by its radius; `None` while a
/// side is unbounded.
pub(crate) fn element_box(e: &Element) -> Option<Aabb> {
    if !(e.lo.is_finite() && e.hi.is_finite()) {
        return None;
    }
    let (p, q) = (
        add(e.point, scale(e.dir, e.lo)),
        add(e.point, scale(e.dir, e.hi)),
    );
    Some((
        [0, 1, 2].map(|i| p[i].min(q[i]) - e.radius),
        [0, 1, 2].map(|i| p[i].max(q[i]) + e.radius),
    ))
}

/// I4: the room between target shaft `a`'s free end and what stops it in
/// neighbour hole `b` beyond it, along `a`'s axis: the first segment of
/// `b`'s stack (among `elements`) too narrow for it, a step, or else the
/// stack's closed end. `None` unless the tip lies in `b` and the stack is
/// closed beyond it, or narrows.
fn to_floor(a: &Element, b: &Element, p: &Pair, tau: f64, elements: &[Element]) -> Option<f64> {
    let tip_high = a.segment + 1 == a.segments
        && a.finish == EndKind::Open
        && a.hi <= p.hi + tau
        && a.hi >= p.lo;
    let tip_low = a.segment == 0 && a.start == EndKind::Open && a.lo >= p.lo - tau && a.lo <= p.hi;
    // A pin with both ends free in one hole: whichever end meets a floor.
    let high = tip_high
        .then(|| floor_beyond(a, b, p, tau, elements, true))
        .flatten();
    let low = tip_low
        .then(|| floor_beyond(a, b, p, tau, elements, false))
        .flatten();
    match (high, low) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

/// I4 beyond one tip of `a` (its high one, or its low): `b`'s next
/// segments on that side, to the first too narrow or the closed end.
fn floor_beyond(
    a: &Element,
    b: &Element,
    p: &Pair,
    tau: f64,
    elements: &[Element],
    high: bool,
) -> Option<f64> {
    // Beyond the tip: on a's high side, b's next segments when b runs the
    // same way, its previous ones when it runs against.
    let forward = high == (dot(a.dir, b.dir) > 0.0);
    let room = |near: f64, far: f64| {
        if high {
            (near - a.hi).max(0.0)
        } else {
            (a.lo - far).max(0.0)
        }
    };
    let (mut segment, mut last) = (b.segment, *p);
    loop {
        let next = if forward {
            Some(segment + 1).filter(|k| *k < b.segments)
        } else {
            segment.checked_sub(1)
        };
        let Some(next) = next else {
            let closed = if forward { b.finish } else { b.start } == EndKind::Closed;
            return closed.then(|| room(last.hi, last.lo));
        };
        let s = elements
            .iter()
            .find(|e| e.feature == b.feature && e.segment == next)?;
        let q = axial::pair(a, s, PARALLEL_SINE_REL, -1.0)?;
        if s.radius < a.radius - tau {
            // A step it cannot pass: its near side stops the shaft.
            return Some(room(q.lo, q.hi));
        }
        segment = next;
        last = q;
    }
}

/// True if `shaft`'s part is a ring clear of the part holding the hole
/// it was paired with, over `p`'s overlap on `shaft`'s axis: `ring` has a
/// bore coaxial with `shaft` and inside it, and `core` (the hole's part)
/// an outside coaxial with it no wider than that bore. The wall `shaft`
/// bounds lies outside that bore and the core inside its outside, so
/// they meet at most, whatever the hole: a spacer inside a bearing's
/// bore. A wider bore of the ring's (a cap's skirt around its spigot)
/// says nothing of this wall; a core with no round outside (a hex nut)
/// proves nothing; and only what is known of a cylinder covers, never
/// an undecided one's.
fn hollow(ring: &[Element], shaft: &Element, core: &[Element], p: &Pair, tau: f64) -> bool {
    let (from, to) = (p.from.min(p.to), p.from.max(p.to));
    let covering = |e: &&Element, side: Side| {
        e.side == side
            && e.partial.is_none()
            && !e.undecided
            && axial::pair(shaft, &known(e), PARALLEL_SINE_REL, 0.0)
                .is_some_and(|q| q.e_max <= tau && q.lo <= from + tau && q.hi >= to - tau)
    };
    // The outermost, should the core have walls within walls.
    let Some(outside) = core
        .iter()
        .filter(|e| covering(e, Side::Shaft))
        .map(|e| e.radius)
        .reduce(f64::max)
    else {
        return false;
    };
    ring.iter()
        .filter(|e| covering(e, Side::Hole) && e.radius < shaft.radius - tau)
        .any(|e| outside <= e.radius + tau)
}

/// U3: what is known of an element, before an unknown side runs on.
fn known(e: &Element) -> Element {
    let mut known = e.clone();
    known.lo = e.known_lo;
    known.hi = e.known_hi;
    known
}

/// X7: an element's own cylinder, without the zones it stands for.
fn own(e: &Element) -> Element {
    let mut own = e.clone();
    own.lo = e.own_lo;
    own.hi = e.own_hi;
    own
}

/// X7: a line-to-line or interference fit holds only where both
/// cylinders themselves overlap; within a stretched transition or end zone
/// it is a cone or step bearing, which the seat rules read (Q, C). Clips
/// `relation` to the cylinders' overlap and returns true, or, with none,
/// keeps it as B6's provenance if it opens a rim (true) or drops it.
fn own_overlap(relation: &mut Relation, a: &Element, b: &Element, grow: f64, tau: f64) -> bool {
    match axial::pair(&own(a), &own(b), PARALLEL_SINE_REL, grow) {
        Some(q) if q.length > tau => {
            relation.span = Some((q.from, q.to));
            true
        }
        _ => {
            relation.rule = "B6";
            relation.fit = None;
            relation.span = None;
            relation.clear = None;
            relation.margin = None;
            relation.flags.remove(&Flag::ThreadNotConfirmed);
            relation.flags.remove(&Flag::Conflict);
            relation.open_start || relation.open_finish
        }
    }
}

/// True if the face's box, moved into the target's frame, may meet the
/// open core of hole element `a`: radius `r − τ`, stations shrunk by `τ`;
/// if so, the stations the box spans. A face with no box may meet
/// anything, anywhere.
fn meets_core(a: &Element, face: &FaceBox, motion: &Motion, tau: f64) -> Option<(f64, f64)> {
    let Some((low, high)) = face.bounds else {
        return Some((f64::NEG_INFINITY, f64::INFINITY));
    };
    let (u, w) = perpendiculars(a.dir);
    let mut s_range = (f64::INFINITY, f64::NEG_INFINITY);
    let mut u_range = s_range;
    let mut w_range = s_range;
    for i in 0..8 {
        let corner: V = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let d = sub(motion.point(corner), a.point);
        let widen = |r: (f64, f64), x: f64| (r.0.min(x), r.1.max(x));
        s_range = widen(s_range, dot(d, a.dir));
        u_range = widen(u_range, dot(d, u));
        w_range = widen(w_range, dot(d, w));
    }
    if s_range.1 <= a.lo + tau || s_range.0 >= a.hi - tau {
        return None;
    }
    // The distance from the axis to the box's footprint across it: no
    // more than to the hull of the moved box, so "may meet" errs toward
    // meeting.
    let nearest = |r: (f64, f64)| {
        if r.0 > 0.0 {
            r.0
        } else if r.1 < 0.0 {
            -r.1
        } else {
            0.0
        }
    };
    (norm([nearest(u_range), nearest(w_range), 0.0]) < a.radius - tau).then_some(s_range)
}

/// True if two boxes, the second grown by `by`, meet.
pub(crate) fn boxes_meet(a: &Extent, b: (V, V), by: f64) -> bool {
    let (al, ah) = (a.min_mm.to_array(), a.max_mm.to_array());
    (0..3).all(|i| al[i] <= b.1[i] + by && b.0[i] - by <= ah[i])
}

/// `extent` moved by `motion`, as the box of its moved corners.
pub(crate) fn moved_box(extent: &Extent, motion: &Motion) -> (V, V) {
    let (low, high) = (extent.min_mm.to_array(), extent.max_mm.to_array());
    let mut out = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for i in 0..8 {
        let p = motion.point([
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ]);
        for ((low, high), x) in out.0.iter_mut().zip(out.1.iter_mut()).zip(p) {
            *low = low.min(x);
            *high = high.max(x);
        }
    }
    out
}

/// The reach of the bounded rules: `NEAR_MM` plus the floor (N3).
pub(crate) fn reach(floor: f64) -> f64 {
    NEAR_MM + floor
}
