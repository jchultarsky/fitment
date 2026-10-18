//! The shortlist, level 1 (`docs/research/m4-match.md`, SG10, SG11, SL7
//! to SL9; `docs/research/m5-catalog.md`, IX): policy-free tokens per
//! candidate, posting lists by sense and diameter bin, and a query whose
//! windows contain level 2's. One algorithm, [`level_one`], runs over any
//! [`IndexSource`]: the in-memory [`ShortlistIndex`], or a catalog's stored
//! postings.
//!
//! What level 1 promises is that it never hides a part verification
//! passes: each filter asks only what a passing part's own witnesses show
//! (a stack with its own diameter in each line's window, distinct planes
//! for the seat groups, a token between the selective line's witness and
//! each other's). It does not promise to return everything level 2 keeps:
//! level 2 keeps untested what its budget cannot test (SL4), and its
//! clusters lend a stack its neighbours' diameters (SL2), where tokens hold
//! each stack's own. Both differences touch only parts verification
//! rejects (the M5 PR 1 soundness review).

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use super::signature::{PartSignature, SocketSignature, Window, line_angle, point_line, slack};
use crate::features::{FormatError, Sense};
use crate::socket::geo::{V, add, scale};

/// The bin width (SG10), whatever the policy (SG11).
const BIN_MM: f64 = 0.1;

/// Pair tokens join stacks whose axes are within this (SL7): 0.1°.
const PAIR_ANGLE_RAD: f64 = 0.1 * std::f64::consts::PI / 180.0;

/// A part with more pair tokens is marked incomplete (SL7).
const MAX_PAIR_TOKENS: usize = 20_000;

/// A part whose stacks take more comparisons to pair is marked incomplete
/// too: the work is bounded.
const MAX_PAIR_COMPARISONS: usize = 1_000_000;

/// SG10: the bin of a length.
fn bin(v: f64) -> i64 {
    // A finite length well inside i64's range: the gate bounds
    // coordinates at 1e6 mm.
    #[allow(clippy::cast_possible_truncation)]
    let b = libm::floor(v / BIN_MM) as i64;
    b
}

/// Two stacks with near-parallel axes (SL7), by their indices in
/// [`IndexTokens::stacks`]: their senses and diameters are the stacks'.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PairToken {
    /// The first stack.
    pub a: u32,
    /// The second stack.
    pub b: u32,
    /// The distance from the first's axis to the second's midpoint.
    pub distance_mm: f64,
}

/// A candidate's level-1 tokens (SL7): policy-free.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IndexTokens {
    /// Each stack's sense and segment diameters.
    pub stacks: Vec<(Sense, Vec<f64>)>,
    /// Ordered pairs of stacks within 0.1°.
    pub pairs: Vec<PairToken>,
    /// The number of planes.
    pub planes: u32,
    /// The lever (SG6).
    pub lever_mm: f64,
    /// The candidate's own uncertainty.
    pub floor_mm: f64,
    /// True if it had more pairs than are kept: the pair filter is not
    /// applied to it.
    pub pairs_incomplete: bool,
}

/// One posted diameter of a candidate's tokens (M5's IX2).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Posting {
    /// The stack's sense.
    pub sense: Sense,
    /// The diameter's bin.
    pub bin: i64,
    /// The stack's index in [`IndexTokens::stacks`].
    pub stack: u32,
    /// The diameter.
    pub diameter_mm: f64,
}

impl IndexTokens {
    /// The tokens of `part`'s signature.
    pub fn of(part: &PartSignature) -> Self {
        let stacks: Vec<(Sense, Vec<f64>)> = part
            .stacks()
            .iter()
            .map(|s| (s.sense, s.diameters_mm.clone()))
            .collect();
        let mid = |s: &super::SigStack| -> V {
            let (p, d) = (s.axis.point_mm.to_array(), s.axis.direction.to_array());
            match s.extent_mm {
                [Some(a), Some(b)] => add(p, scale(d, f64::midpoint(a, b))),
                _ => p,
            }
        };
        let mut pairs = Vec::new();
        let mut incomplete = false;
        let mut compared = 0_usize;
        let index = |k: usize| u32::try_from(k).unwrap_or(u32::MAX);
        'pairs: for (i, a) in part.stacks().iter().enumerate() {
            for (j, b) in part.stacks().iter().enumerate() {
                compared += 1;
                if compared > MAX_PAIR_COMPARISONS {
                    incomplete = true;
                    break 'pairs;
                }
                if a.feature == b.feature {
                    continue;
                }
                let (pa, da) = (a.axis.point_mm.to_array(), a.axis.direction.to_array());
                if line_angle(da, b.axis.direction.to_array()) > PAIR_ANGLE_RAD {
                    continue;
                }
                if pairs.len() >= MAX_PAIR_TOKENS {
                    incomplete = true;
                    break 'pairs;
                }
                pairs.push(PairToken {
                    a: index(i),
                    b: index(j),
                    distance_mm: point_line(mid(b), pa, da),
                });
            }
        }
        Self {
            stacks,
            pairs,
            planes: u32::try_from(part.planes().len()).unwrap_or(u32::MAX),
            lever_mm: part.lever_mm(),
            floor_mm: part.uncertainty_mm(),
            pairs_incomplete: incomplete,
        }
    }

    /// The tokens as compact JSON: a catalog's stored form.
    pub fn to_json(&self) -> String {
        // Every float is finite by construction and every key a string.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads tokens back (M5's IX9): every length finite and not negative,
    /// `-0.0` included, every diameter above zero, and every pair naming two
    /// stacks there are, in the order [`IndexTokens::of`] writes them.
    ///
    /// # Errors
    ///
    /// A [`FormatError`] if the text is not tokens this crate writes.
    pub fn from_json(text: &str) -> Result<Self, FormatError> {
        let tokens: Self = serde_json::from_str(text).map_err(|error| FormatError::Json {
            message: error.to_string(),
            line: error.line(),
            column: error.column(),
        })?;
        let invalid = |message: &str| Err(FormatError::Invalid(message.to_owned()));
        // Not `-0.0` either: a store may keep it as `0.0` (IX3).
        let length = |v: f64| v.is_finite() && v.is_sign_positive();
        if !length(tokens.lever_mm) || !length(tokens.floor_mm) {
            return invalid("a lever or floor that is not a finite length");
        }
        if !tokens
            .stacks
            .iter()
            .all(|(_, diameters)| diameters.iter().all(|d| length(*d) && *d > 0.0))
        {
            return invalid("a diameter that is not a finite length above zero");
        }
        let n = tokens.stacks.len();
        if !tokens.pairs.iter().all(|p| {
            (p.a as usize) < n && (p.b as usize) < n && p.a != p.b && length(p.distance_mm)
        }) {
            return invalid("a pair that names no two stacks, or no finite distance");
        }
        if !tokens.pairs.windows(2).all(|w| match w {
            [x, y] => (x.a, x.b) < (y.a, y.b),
            _ => true,
        }) {
            return invalid("pairs out of their order");
        }
        Ok(tokens)
    }

    /// Every posted diameter (IX2), stack by stack.
    pub fn postings(&self) -> impl Iterator<Item = Posting> + '_ {
        self.stacks
            .iter()
            .enumerate()
            .flat_map(|(k, (sense, diameters))| {
                diameters.iter().map(move |d| Posting {
                    sense: *sense,
                    bin: bin(*d),
                    stack: u32::try_from(k).unwrap_or(u32::MAX),
                    diameter_mm: *d,
                })
            })
    }
}

/// Why a query returns no candidate (SL8).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Blocked {
    /// The socket demands nothing a candidate can show.
    Empty,
    /// Some active requirement fails every candidate (SG7).
    #[non_exhaustive]
    FailsEvery {
        /// Those requirements.
        requirements: Vec<crate::verdict::RequirementId>,
    },
}

/// Where level 1's postings live (M5's IX4): the in-memory
/// [`ShortlistIndex`], or a catalog's stored tables. [`level_one`] runs
/// over either, so both answer alike.
pub trait IndexSource {
    /// The store's error.
    type Error;

    /// Every posting of `sense` whose bin lies in `bins`, as (candidate,
    /// stack, diameter), in any order.
    ///
    /// # Errors
    ///
    /// The store's, if it cannot be read.
    fn postings(
        &self,
        sense: Sense,
        bins: RangeInclusive<i64>,
        each: &mut dyn FnMut(u64, u32, f64),
    ) -> Result<(), Self::Error>;

    /// The tokens of each of `ids`, or of every indexed candidate for
    /// `None`, in any order. Every id asked for is answered or is the
    /// store's error: an id skipped is a candidate level 1 drops though
    /// level 2 might keep it.
    ///
    /// # Errors
    ///
    /// The store's, if it cannot be read or holds no tokens for an id.
    fn tokens(
        &self,
        ids: Option<&[u64]>,
        each: &mut dyn FnMut(u64, &IndexTokens),
    ) -> Result<(), Self::Error>;
}

/// The bins a window's postings lie in (SG10), one more at each end; an
/// unbounded end reaches every bin.
fn bins(window: &Window) -> RangeInclusive<i64> {
    let lo = window.lo.map_or(i64::MIN, |v| bin(v).saturating_sub(1));
    let hi = window.hi.map_or(i64::MAX, |v| bin(v).saturating_add(1));
    lo..=hi
}

/// IX5's seat bucket: the lever rounded up to a power of two, never below
/// it (rounding `2^ceil(log2(x))` can come out one ulp below `x` just past
/// a power of two). Groups only coarsen as the lever grows, so a bucket
/// asks no more than level 2 does at the part's own lever.
pub(super) fn seat_bucket(lever_mm: f64) -> f64 {
    if lever_mm > 0.0 {
        libm::exp2(libm::ceil(libm::log2(lever_mm))).max(lever_mm)
    } else {
        lever_mm
    }
}

/// SL8 over any [`IndexSource`] (M5's IX4 to IX8): the candidates level 2
/// may keep for `socket`, ascending, among `within` if given (another
/// socket's answer, say). Each candidate is judged at its own lever,
/// bucketed (IX5), so its answer depends on no other candidate.
///
/// # Errors
///
/// The source's error; the inner [`Blocked`] if the socket demands
/// nothing, or has an active requirement that fails every candidate.
pub fn level_one<S: IndexSource>(
    socket: &SocketSignature,
    source: &S,
    within: Option<&[u64]>,
) -> Result<Result<Vec<u64>, Blocked>, S::Error> {
    if !socket.blocked_by().is_empty() {
        return Ok(Err(Blocked::FailsEvery {
            requirements: socket.blocked_by().to_vec(),
        }));
    }
    if socket.is_empty() {
        return Ok(Err(Blocked::Empty));
    }
    // Per required line, its narrowest window: some stack of the sense with
    // a diameter in it. Not a count of distinct stacks: two witnesses may
    // take their diameters from one neighbour (SL2).
    let mut needs: BTreeMap<(u8, [u64; 2]), (Sense, Window)> = BTreeMap::new();
    for line in &socket.lines {
        if let Some(w) = narrowest(&line.windows) {
            let key = (
                u8::from(line.sense == Sense::Shaft),
                [w.lo, w.hi].map(|v| v.unwrap_or(f64::NAN).to_bits()),
            );
            needs.entry(key).or_insert((line.sense, w));
        }
    }
    let mut needs: Vec<(Sense, Window)> = needs.into_values().collect();
    needs.sort_by(|a, b| a.1.width().total_cmp(&b.1.width()));
    let mut found: Option<Vec<u64>> = within.map(|ids| {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        ids
    });
    if found.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Ok(Vec::new()));
    }
    for (sense, window) in needs {
        let mut ids: Vec<u64> = Vec::new();
        source.postings(sense, bins(&window), &mut |id, _, d| {
            if window.holds(d) {
                ids.push(id);
            }
        })?;
        ids.sort_unstable();
        ids.dedup();
        found = Some(match found.take() {
            None => ids,
            Some(f) => f
                .into_iter()
                .filter(|id| ids.binary_search(id).is_ok())
                .collect(),
        });
        if found.as_ref().is_some_and(Vec::is_empty) {
            return Ok(Ok(Vec::new()));
        }
    }
    let mut groups: BTreeMap<u64, Groups> = BTreeMap::new();
    let mut kept = Vec::new();
    source.tokens(found.as_deref(), &mut |id, t| {
        let lever = seat_bucket(socket.lever_mm.max(t.lever_mm));
        let g = groups
            .entry(lever.to_bits())
            .or_insert_with(|| Groups::at(socket, lever));
        if t.planes as usize >= g.seats && (t.pairs_incomplete || pairs_hold(socket, t, lever, g)) {
            kept.push(id);
        }
    })?;
    kept.sort_unstable();
    kept.dedup();
    Ok(Ok(kept))
}

/// SG5's groups of a socket at one lever: the required lines', and how
/// many groups of seats.
pub(super) struct Groups {
    lines: Vec<usize>,
    pub(super) seats: usize,
}

impl Groups {
    pub(super) fn at(socket: &SocketSignature, lever: f64) -> Self {
        let (lines, mut seats) =
            socket.groups(socket.linear_window(lever), socket.angular_window() + 1e-9);
        seats.sort_unstable();
        seats.dedup();
        Self {
            lines,
            seats: seats.len(),
        }
    }
}

/// SL8: for each parallel pair with the most selective line, a pair token
/// in range; bypassed above 0.05° (the tokens stop at 0.1°). A line of the
/// selective line's SG5 group asks for no pair: one stack may serve both,
/// as level 2 allows, and tokens pair only distinct stacks. `lever` is at
/// least level 2's for this candidate, so the reach and the groups contain
/// level 2's.
fn pairs_hold(socket: &SocketSignature, tokens: &IndexTokens, lever: f64, g: &Groups) -> bool {
    // Level 2's angular window is W_A + 1e-9: past the tokens' 0.1°, a
    // passing part's witnesses may be paired by no token.
    if socket.angular_window() + 1e-9 > PAIR_ANGLE_RAD {
        return true;
    }
    let selective = socket
        .lines
        .iter()
        .enumerate()
        .filter_map(|(k, l)| narrowest(&l.windows).map(|w| (k, l, w)))
        .min_by(|a, b| a.2.width().total_cmp(&b.2.width()));
    let Some((at, first, wa)) = selective else {
        return true;
    };
    let group = g.lines.get(at).copied();
    // Level 2's window, and the drift of a midpoint along a line up to 0.1°
    // off over the lever.
    let reach = socket.linear_window(lever) + lever * libm::sin(PAIR_ANGLE_RAD);
    let stack = |k: u32| tokens.stacks.get(k as usize);
    socket
        .lines
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != at && g.lines.get(*k).copied() != group)
        .all(|(_, other)| {
            if line_angle(first.dir, other.dir) > 1e-9 {
                return true;
            }
            let Some(wb) = narrowest(&other.windows) else {
                return true;
            };
            let distance = point_line(other.mid(), first.point, first.dir);
            tokens.pairs.iter().any(|t| {
                let (Some((sa, da)), Some((sb, db))) = (stack(t.a), stack(t.b)) else {
                    return false;
                };
                [*sa, *sb] == [first.sense, other.sense]
                    && da.iter().any(|d| wa.holds(*d))
                    && db.iter().any(|d| wb.holds(*d))
                    && (t.distance_mm - distance).abs() <= reach + slack(distance)
            })
        })
}

/// One posted diameter in memory: the candidate, its stack, the diameter.
type Posted = (u64, u32, f64);

/// Level 1's index in memory (SL8): diameters posted by sense and bin, with
/// each candidate's tokens for the pair and plane filters.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ShortlistIndex {
    /// (sense, bin) → (candidate, stack, diameter).
    postings: BTreeMap<(u8, i64), Vec<Posted>>,
    tokens: BTreeMap<u64, IndexTokens>,
}

impl ShortlistIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexes candidate `id`.
    pub fn insert(&mut self, id: u64, tokens: &IndexTokens) {
        for p in tokens.postings() {
            self.postings
                .entry((u8::from(p.sense == Sense::Shaft), p.bin))
                .or_default()
                .push((id, p.stack, p.diameter_mm));
        }
        self.tokens.insert(id, tokens.clone());
    }

    /// SL8: [`level_one`] over this index.
    ///
    /// # Errors
    ///
    /// [`Blocked`] if the socket demands nothing, or has an active
    /// requirement that fails every candidate.
    pub fn query(&self, socket: &SocketSignature) -> Result<Vec<u64>, Blocked> {
        match level_one(socket, self, None) {
            Ok(answer) => answer,
            Err(never) => match never {},
        }
    }
}

impl IndexSource for ShortlistIndex {
    type Error = std::convert::Infallible;

    fn postings(
        &self,
        sense: Sense,
        bins: RangeInclusive<i64>,
        each: &mut dyn FnMut(u64, u32, f64),
    ) -> Result<(), Self::Error> {
        if bins.is_empty() {
            return Ok(());
        }
        let key = u8::from(sense == Sense::Shaft);
        for (_, posted) in self
            .postings
            .range((key, *bins.start())..=(key, *bins.end()))
        {
            for &(id, k, d) in posted {
                each(id, k, d);
            }
        }
        Ok(())
    }

    fn tokens(
        &self,
        ids: Option<&[u64]>,
        each: &mut dyn FnMut(u64, &IndexTokens),
    ) -> Result<(), Self::Error> {
        // Every posted id has its tokens: `insert` writes both.
        match ids {
            None => self.tokens.iter().for_each(|(id, t)| each(*id, t)),
            Some(ids) => {
                for id in ids {
                    if let Some(t) = self.tokens.get(id) {
                        each(*id, t);
                    }
                }
            }
        }
        Ok(())
    }
}

/// The narrowest of a line's windows.
fn narrowest(windows: &[Window]) -> Option<Window> {
    windows
        .iter()
        .copied()
        .min_by(|a, b| a.width().total_cmp(&b.width()))
}

#[cfg(test)]
mod tests {
    use super::{BIN_MM, IndexSource, IndexTokens, ShortlistIndex, Window, bin, bins, seat_bucket};
    use crate::features::Sense;

    /// SG10: a value inside a window is in a bin between the window's ends'.
    #[test]
    fn bins_are_monotone_on_their_edges() {
        for k in -50_i32..50 {
            let v = f64::from(k) * BIN_MM;
            for x in [v, v + 1e-12, v - 1e-12, f64::from(k) * 0.1] {
                assert!(bin(x - 1e-9) <= bin(x) && bin(x) <= bin(x + 1e-9));
            }
        }
    }

    /// IX5: a seat bucket is never below the lever, just past a power of
    /// two as much as anywhere.
    #[test]
    fn a_seat_bucket_is_never_below_the_lever() {
        for k in -10_i32..40 {
            let p = libm::exp2(f64::from(k));
            // `f64::next_up` is newer than the MSRV: one ulp by its bits.
            let (up, down) = (
                f64::from_bits(p.to_bits() + 1),
                f64::from_bits(p.to_bits() - 1),
            );
            for x in [p, up, down, p * 1.5] {
                let b = seat_bucket(x);
                assert!(b >= x, "{x}: {b}");
                assert!(b <= 2.0 * x, "{x}: {b}");
            }
        }
        assert!(seat_bucket(0.0).abs() < f64::MIN_POSITIVE);
    }

    /// The lengths CT4 names: every tenth of a millimetre and one ulp either
    /// side, and the extremes the gate admits; a diameter is above zero.
    fn edges() -> Vec<f64> {
        let mut out = vec![1e-6, 2e6, f64::from_bits(1)];
        for k in 1_i32..400 {
            let v = f64::from(k) * BIN_MM;
            out.extend([
                v,
                f64::from_bits(v.to_bits() + 1),
                f64::from_bits(v.to_bits() - 1),
            ]);
        }
        out
    }

    /// M5's CT4, in memory: a diameter round-trips bit for bit through the
    /// stored tokens, and a window's postings are found exactly when the
    /// window holds the diameter, its ends pinned to the diameter, an ulp
    /// off it, or unbounded.
    #[test]
    fn a_window_finds_its_diameters_on_the_bin_edges() {
        for d in edges() {
            let tokens = IndexTokens {
                stacks: vec![(Sense::Hole, vec![d])],
                pairs: Vec::new(),
                planes: 0,
                lever_mm: 1.0,
                floor_mm: 1e-6,
                pairs_incomplete: false,
            };
            let Ok(back) = IndexTokens::from_json(&tokens.to_json()) else {
                unreachable!("{d}: tokens refused");
            };
            assert_eq!(back.stacks[0].1[0].to_bits(), d.to_bits(), "{d}");
            let mut index = ShortlistIndex::new();
            index.insert(1, &back);
            let (below, above) = (
                f64::from_bits(d.to_bits().saturating_sub(1)),
                f64::from_bits(d.to_bits() + 1),
            );
            for (lo, hi) in [
                (Some(d), Some(d)),
                (Some(below), Some(below)),
                (Some(above), Some(above)),
                (Some(below), Some(above)),
                (None, Some(d)),
                (Some(d), None),
                (None, Some(below)),
                (Some(above), None),
                (None, None),
            ] {
                let window = Window { lo, hi };
                let mut found = false;
                let Ok(()) = index.postings(Sense::Hole, bins(&window), &mut |_, _, v| {
                    found |= window.holds(v);
                });
                assert_eq!(found, window.holds(d), "{d} in {lo:?}..{hi:?}");
            }
        }
    }
}
