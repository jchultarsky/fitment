//! Sockets and merging (`docs/research/m3-socket.md`, 9.1): occurrences
//! with the same requirement ids and the same features left out, whose
//! every value agrees within `MERGE_MM` (lengths) or `MERGE_SINE`
//! (directions and angles), share one socket. Values are bucketed by
//! their quantized form, so merging is n log n; a value on a bin edge only
//! costs an unmerged socket. Merged values are the strictest of the
//! members; sources and touchers are their union.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::model::{Requirement, Requires, Socket};
use super::{MERGE_MM, MERGE_SINE};

/// One value of a signature: a quantized number, or anything else exactly.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Atom {
    Number(i64),
    Text(String),
}

/// The keys whose numbers are directions or angles.
const ANGULAR: [&str; 6] = [
    "direction",
    "normal",
    "included_angle_rad",
    "sweep_rad",
    "sine",
    "rotation",
];

/// The fields that name neighbours, or carry them: not values of the
/// socket's own.
const SKIPPED: [&str; 3] = ["sources", "neighbour", "by"];

fn atoms(value: &Value, quantum: f64, out: &mut Vec<Atom>) {
    match value {
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(f64::NAN);
            #[allow(clippy::cast_possible_truncation)]
            let q = (x / quantum).round() as i64;
            out.push(Atom::Number(q));
        }
        Value::Array(items) => {
            out.push(Atom::Text(format!("[{}", items.len())));
            for item in items {
                atoms(item, quantum, out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if key == "by" {
                    // How many touch, not which.
                    let count = item.as_array().map_or(0, Vec::len);
                    out.push(Atom::Text(format!("by{count}")));
                    continue;
                }
                if SKIPPED.contains(&key.as_str()) {
                    continue;
                }
                out.push(Atom::Text(key.clone()));
                let q = if ANGULAR.contains(&key.as_str()) {
                    MERGE_SINE
                } else {
                    quantum
                };
                atoms(item, q, out);
            }
        }
        other => out.push(Atom::Text(other.to_string())),
    }
}

/// What must agree for two occurrences to share a socket.
fn signature(socket: &Socket) -> Vec<Atom> {
    let mut out = Vec::new();
    for requirement in &socket.requirements {
        out.push(Atom::Text(requirement.id.clone()));
        let value = serde_json::to_value(&requirement.requires).unwrap_or(Value::Null);
        atoms(&value, MERGE_MM, &mut out);
        let flags = serde_json::to_value(&requirement.flags).unwrap_or(Value::Null);
        atoms(&flags, MERGE_MM, &mut out);
    }
    let left = serde_json::to_value(&socket.left_out).unwrap_or(Value::Null);
    atoms(&left, MERGE_MM, &mut out);
    out
}

/// One socket per occurrence in, merged sockets out: numbered by their
/// smallest occurrence path, each listing its occurrences in order.
pub(crate) fn merge(sockets: Vec<Socket>) -> Vec<Socket> {
    let mut buckets: BTreeMap<Vec<Atom>, Vec<Socket>> = BTreeMap::new();
    for socket in sockets {
        buckets.entry(signature(&socket)).or_default().push(socket);
    }
    let mut merged: Vec<Socket> = buckets
        .into_values()
        .filter_map(|members| {
            let mut members = members.into_iter();
            let mut first = members.next()?;
            for other in members {
                absorb(&mut first, other);
            }
            first.occurrences.sort();
            first.occurrences.dedup();
            for requirement in &mut first.requirements {
                normalize(requirement);
            }
            Some(first)
        })
        .collect();
    merged.sort_by(|a, b| a.occurrences.first().cmp(&b.occurrences.first()));
    for (k, socket) in merged.iter_mut().enumerate() {
        socket.id = format!("socket-{}", k + 1);
    }
    merged
}

/// The document's order for a requirement's lists (9.4 checks it):
/// sources by occurrence, neighbour, rule, feature and faces; contacts by
/// face; touchers by neighbour and face; rings by hole, neighbour and
/// diameters; grip holes and cause faces by `#id`; each unique.
pub(crate) fn normalize(requirement: &mut Requirement) {
    let key = |s: &super::model::SourceRef| {
        (
            s.occurrence.clone(),
            s.neighbour.clone(),
            s.rule.clone(),
            s.feature.map(crate::features::FeatureId::instance),
            s.faces.clone(),
        )
    };
    requirement.sources.sort_by_key(key);
    requirement.sources.dedup_by(|a, b| key(a) == key(b));
    match &mut requirement.requires {
        Requires::Seat {
            contacts, rings, ..
        } => {
            contacts.sort_by_key(|c| c.face);
            for c in contacts.iter_mut() {
                c.by.sort_by(|x, y| (&x.neighbour, x.face).cmp(&(&y.neighbour, y.face)));
                c.by.dedup_by(|x, y| x.neighbour == y.neighbour && x.face == y.face);
            }
            rings.sort_by(|x, y| {
                (x.hole.instance(), &x.neighbour)
                    .cmp(&(y.hole.instance(), &y.neighbour))
                    .then(x.inner_diameter_mm.total_cmp(&y.inner_diameter_mm))
                    .then(x.outer_diameter_mm.total_cmp(&y.outer_diameter_mm))
            });
            rings.dedup();
        }
        Requires::Grip { holes, .. } => {
            holes.sort_by_key(|h| h.instance());
            holes.dedup();
        }
        Requires::Unresolved { causes, .. } => {
            for c in causes.iter_mut() {
                c.faces.sort_unstable();
                c.faces.dedup();
            }
        }
        _ => {}
    }
}

/// `other`'s occurrences and requirements into `socket`, the strictest of
/// each value kept.
fn absorb(socket: &mut Socket, other: Socket) {
    socket.occurrences.extend(other.occurrences);
    for (ours, theirs) in socket.requirements.iter_mut().zip(other.requirements) {
        strictest(ours, theirs);
    }
}

fn strictest(ours: &mut Requirement, theirs: Requirement) {
    ours.floor_mm = ours.floor_mm.max(theirs.floor_mm);
    let flags: BTreeSet<_> = ours.flags.iter().chain(&theirs.flags).copied().collect();
    ours.flags = flags.into_iter().collect();
    // Lists are gathered here and sorted once, in `normalize`, after every
    // member is absorbed: sorting on each absorb made merging quadratic.
    ours.sources.extend(theirs.sources);
    match (&mut ours.requires, theirs.requires) {
        (
            Requires::Bore { spans, open, .. },
            Requires::Bore {
                spans: other,
                open: their_open,
                ..
            },
        ) => {
            for (a, b) in spans.iter_mut().zip(other) {
                // A bore must pass the larger of the occupants.
                a.clear_diameter_mm = match (a.clear_diameter_mm, b.clear_diameter_mm) {
                    (Some(x), Some(y)) => Some(x.max(y)),
                    (x, y) => x.or(y),
                };
                a.pinned |= b.pinned;
            }
            open.start |= their_open.start;
            open.finish |= their_open.finish;
        }
        (
            Requires::Shaft {
                spans,
                max_extra_mm,
                ..
            },
            Requires::Shaft {
                spans: other,
                max_extra_mm: their_extra,
                ..
            },
        ) => {
            for (a, b) in spans.iter_mut().zip(other) {
                // A shaft must fit the smaller of the holes; pinned, none.
                a.pinned |= b.pinned;
                a.clear_diameter_mm = match (a.clear_diameter_mm, b.clear_diameter_mm) {
                    _ if a.pinned => None,
                    (Some(x), Some(y)) => Some(x.min(y)),
                    _ => None,
                };
            }
            *max_extra_mm = match (*max_extra_mm, their_extra) {
                (Some(x), Some(y)) => Some(x.min(y)),
                _ => None,
            };
        }
        (
            Requires::Seat {
                contacts, rings, ..
            },
            Requires::Seat {
                contacts: other,
                rings: their_rings,
                ..
            },
        ) => {
            for (a, b) in contacts.iter_mut().zip(other) {
                a.by.extend(b.by);
            }
            rings.extend(their_rings);
        }
        (Requires::Unresolved { causes, .. }, Requires::Unresolved { causes: other, .. }) => {
            for (a, b) in causes.iter_mut().zip(other) {
                a.faces.extend(b.faces);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::SocketDocument;
    use crate::socket::model::{Flag, LeftOut};

    const S01: &[u8] = include_bytes!("../../tests/assemblies/s01_bolted.stp");

    /// s01's bolt socket, for one occurrence path.
    fn one(path: u64) -> Option<Socket> {
        let document = SocketDocument::extract(S01, Some("BOLT"), None).ok()?;
        let mut socket = document.extracted()?.sockets.first()?.clone();
        socket.occurrences = vec![vec![path]];
        Some(socket)
    }

    fn count(sockets: Vec<Socket>) -> usize {
        merge(sockets).len()
    }

    #[test]
    fn only_sockets_that_agree_merge() {
        let (Some(a), Some(b)) = (one(1), one(2)) else {
            return;
        };
        assert_eq!(count(vec![a.clone(), b.clone()]), 1);
        // One toucher fewer on a seat.
        let mut fewer = b.clone();
        for r in &mut fewer.requirements {
            if let Requires::Seat { contacts, .. } = &mut r.requires {
                for c in contacts {
                    c.by.clear();
                }
            }
        }
        assert_eq!(count(vec![a.clone(), fewer]), 2);
        // A flag more.
        let mut flagged = b.clone();
        for r in &mut flagged.requirements {
            r.flags.push(Flag::Offset);
            r.flags.sort();
        }
        assert_eq!(count(vec![a.clone(), flagged]), 2);
        // A feature left out.
        let mut left = b.clone();
        let shaft = left.requirements.iter().find_map(|r| match &r.requires {
            Requires::Shaft { shaft, axis, .. } => Some((*shaft, *axis)),
            _ => None,
        });
        if let Some((feature, axis)) = shaft {
            left.left_out.push(LeftOut {
                feature,
                reason: crate::socket::model::LeftOutReason::NothingOnAxis,
                axis,
                diameter_mm: 1.0,
                from_mm: 0.0,
                to_mm: 1.0,
            });
        }
        assert!(shaft.is_some());
        assert_eq!(count(vec![a, left]), 2);
    }

    #[test]
    fn sockets_are_numbered_by_their_smallest_occurrence() {
        let (Some(a), Some(b), Some(c)) = (one(5), one(1), one(3)) else {
            return;
        };
        let mut odd = c;
        for r in &mut odd.requirements {
            r.flags.push(Flag::Offset);
            r.flags.sort();
        }
        // {1, 5} and {3}: the first holds path 1.
        let merged = merge(vec![a, odd, b]);
        assert_eq!(merged.len(), 2);
        assert!(
            merged
                .first()
                .is_some_and(|s| s.id == "socket-1" && s.occurrences == vec![vec![1], vec![5]])
        );
        assert!(
            merged
                .get(1)
                .is_some_and(|s| s.id == "socket-2" && s.occurrences == vec![vec![3]])
        );
    }
}
