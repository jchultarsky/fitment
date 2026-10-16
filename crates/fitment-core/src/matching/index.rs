//! The shortlist, level 1 (`docs/research/m4-match.md`, SG10, SG11, SL7
//! to SL9): policy-free tokens per candidate, posting lists by sense and
//! diameter bin, and a query whose windows contain level 2's, so whatever
//! level 2 keeps, level 1 returns. M5 stores the tokens; data only here.

use std::collections::BTreeMap;

use serde::Serialize;

use super::signature::{PartSignature, SocketSignature, Window, line_angle, point_line, slack};
use crate::features::Sense;
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

/// Two stacks with near-parallel axes (SL7).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct PairToken {
    /// The two stacks' senses.
    pub senses: [Sense; 2],
    /// Their segment diameters.
    pub diameters: [Vec<f64>; 2],
    /// The distance from the first's axis to the second's midpoint.
    pub distance_mm: f64,
}

/// A candidate's level-1 tokens (SL7): policy-free.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
        'pairs: for a in part.stacks() {
            for b in part.stacks() {
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
                    senses: [a.sense, b.sense],
                    diameters: [a.diameters_mm.clone(), b.diameters_mm.clone()],
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
}

/// Why a query returns no candidate (SL8).
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

/// One posted diameter: the candidate, its stack, the diameter.
type Posting = (u64, usize, f64);

/// Level 1's index (SL8): diameters posted by sense and bin, with each
/// candidate's tokens for the pair and plane filters.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ShortlistIndex {
    /// (sense, bin) → (candidate, stack, diameter).
    postings: BTreeMap<(u8, i64), Vec<Posting>>,
    tokens: BTreeMap<u64, IndexTokens>,
    /// The largest lever indexed.
    lever_mm: f64,
}

fn sense_key(sense: Sense) -> u8 {
    u8::from(sense == Sense::Shaft)
}

impl ShortlistIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexes candidate `id`.
    pub fn insert(&mut self, id: u64, tokens: &IndexTokens) {
        for (k, (sense, diameters)) in tokens.stacks.iter().enumerate() {
            for d in diameters {
                self.postings
                    .entry((sense_key(*sense), bin(*d)))
                    .or_default()
                    .push((id, k, *d));
            }
        }
        self.lever_mm = self.lever_mm.max(tokens.lever_mm);
        self.tokens.insert(id, tokens.clone());
    }

    /// The candidates whose stacks of `sense` hold a diameter in `window`,
    /// each with the number of such stacks.
    fn holding(&self, sense: Sense, window: &Window) -> BTreeMap<u64, usize> {
        let key = sense_key(sense);
        let lo = window.lo.map_or(i64::MIN, |v| bin(v).saturating_sub(1));
        let hi = window.hi.map_or(i64::MAX, |v| bin(v).saturating_add(1));
        let mut stacks: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for (_, posted) in self.postings.range((key, lo)..=(key, hi)) {
            for &(id, k, d) in posted {
                if window.holds(d) {
                    stacks.entry(id).or_default().push(k);
                }
            }
        }
        stacks
            .into_iter()
            .map(|(id, mut k)| {
                k.sort_unstable();
                k.dedup();
                (id, k.len())
            })
            .collect()
    }

    /// SL8: the candidates level 2 may keep for `socket`, sorted.
    ///
    /// # Errors
    ///
    /// [`Blocked`] if the socket demands nothing, or has an active
    /// requirement that fails every candidate.
    pub fn query(&self, socket: &SocketSignature) -> Result<Vec<u64>, Blocked> {
        if !socket.blocked_by().is_empty() {
            return Err(Blocked::FailsEvery {
                requirements: socket.blocked_by().to_vec(),
            });
        }
        if socket.is_empty() {
            return Err(Blocked::Empty);
        }
        let mut found: Option<Vec<u64>> = None;
        let mut keep = |ids: Vec<u64>| {
            found = Some(match found.take() {
                None => ids,
                Some(f) => f
                    .into_iter()
                    .filter(|id| ids.binary_search(id).is_ok())
                    .collect(),
            });
        };
        // Per required line, its narrowest window: some stack of the sense
        // with a diameter in it. Not a count of distinct stacks: two
        // witnesses may take their diameters from one neighbour (SL2).
        let mut needs: BTreeMap<(u8, [u64; 2]), (Sense, Window)> = BTreeMap::new();
        for line in &socket.lines {
            if let Some(w) = narrowest(&line.windows) {
                let key = (
                    sense_key(line.sense),
                    [w.lo, w.hi].map(|v| v.unwrap_or(f64::NAN).to_bits()),
                );
                needs.entry(key).or_insert((line.sense, w));
            }
        }
        for (sense, window) in needs.into_values() {
            keep(self.holding(sense, &window).into_keys().collect());
        }
        // Seats one plane may serve together count once (SG5).
        let lever = socket.lever_mm.max(self.lever_mm);
        let (_, groups) =
            socket.groups(socket.linear_window(lever), socket.angular_window() + 1e-9);
        let mut groups = groups;
        groups.sort_unstable();
        groups.dedup();
        let seats = groups.len();
        let all: Vec<u64> = found.unwrap_or_else(|| self.tokens.keys().copied().collect());
        Ok(all
            .into_iter()
            .filter(|id| {
                self.tokens.get(id).is_some_and(|t| {
                    t.planes as usize >= seats && (t.pairs_incomplete || self.pairs_hold(socket, t))
                })
            })
            .collect())
    }

    /// SL8: for each parallel pair with the most selective line, a pair
    /// token in range; bypassed above 0.05° (the tokens stop at 0.1°).
    fn pairs_hold(&self, socket: &SocketSignature, tokens: &IndexTokens) -> bool {
        if socket.angular_window() > PAIR_ANGLE_RAD {
            return true;
        }
        let selective = socket
            .lines
            .iter()
            .filter_map(|l| narrowest(&l.windows).map(|w| (l, w)))
            .min_by(|a, b| a.1.width().total_cmp(&b.1.width()));
        let Some((first, wa)) = selective else {
            return true;
        };
        let lever = socket.lever_mm.max(self.lever_mm);
        // Level 2's window, and the drift of a midpoint along a line up to
        // 0.1° off over the lever.
        let reach = socket.linear_window(lever) + lever * libm::sin(PAIR_ANGLE_RAD);
        socket
            .lines
            .iter()
            .filter(|l| !std::ptr::eq(*l, first))
            .all(|other| {
                if line_angle(first.dir, other.dir) > 1e-9 {
                    return true;
                }
                let Some(wb) = narrowest(&other.windows) else {
                    return true;
                };
                let distance = point_line(other.mid(), first.point, first.dir);
                tokens.pairs.iter().any(|t| {
                    t.senses == [first.sense, other.sense]
                        && t.diameters[0].iter().any(|d| wa.holds(*d))
                        && t.diameters[1].iter().any(|d| wb.holds(*d))
                        && (t.distance_mm - distance).abs() <= reach + slack(distance)
                })
            })
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
    use super::{BIN_MM, bin};

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
}
