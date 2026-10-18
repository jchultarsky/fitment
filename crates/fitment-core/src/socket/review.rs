//! Review decisions (`docs/research/m3-socket.md`, 9.4) and carrying them
//! over to a document extracted again from the same file (9.5).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::check::MAX_NOTE_CHARS;
use super::model::{Extracted, Requirement, ReviewEntry, ReviewStatus, Socket};

/// Why a decision could not be carried over.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StaleReason {
    /// The requirement's values, floor or flags changed.
    RequirementChanged,
    /// No socket of the new document holds it for those occurrences.
    RequirementGone,
    /// The occurrences now merged had different decisions, or some none.
    OccurrencesDisagree,
}

/// A decision that could not be carried over, kept for the user to see.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StaleReview {
    /// The socket it was made in, in the old document.
    pub socket: String,
    /// The requirement.
    pub requirement: String,
    /// The decision.
    pub status: ReviewStatus,
    /// Its note.
    #[serde(deserialize_with = "Option::deserialize")]
    pub note: Option<String>,
    /// Why it was not carried over.
    pub reason: StaleReason,
}

/// What carrying decisions over did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CarryOver {
    /// Decisions applied to the new document's requirements.
    pub carried: usize,
    /// Decisions that went to `stale_reviews`.
    pub stale: usize,
}

/// Why a review decision cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReviewError {
    /// No socket holds the requirement (or not the one named).
    #[error("no socket holds '{0}'")]
    UnknownId(String),
    /// The socket named does not exist.
    #[error("there is no socket '{0}'")]
    UnknownSocket(String),
    /// The requirement differs between the sockets holding it: name one.
    #[error("'{id}' differs between {}: name one with --socket", sockets.join(", "))]
    DiffersBetweenSockets {
        /// The requirement.
        id: String,
        /// The sockets holding it.
        sockets: Vec<String>,
    },
    /// A removal needs a note.
    #[error("removing '{0}' needs a note")]
    NoteRequired(String),
    /// The note is too long.
    #[error("the note is longer than {MAX_NOTE_CHARS} characters")]
    NoteTooLong,
    /// The document holds no sockets.
    #[error("a refused document has nothing to review")]
    Refused,
}

/// What must be equal for a decision to carry over: the requirement
/// without its sources.
fn values(requirement: &Requirement) -> serde_json::Value {
    serde_json::to_value((
        &requirement.requires,
        requirement.floor_mm,
        &requirement.flags,
    ))
    .unwrap_or_default()
}

/// A socket of an earlier document, as carrying its decisions over needs
/// it: each requirement's id and values (`values`), as JSON, so a document
/// of older socket rules is read too (`docs/research/m4-match.md`, MF11).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PriorSocket {
    pub(crate) id: String,
    pub(crate) occurrences: Vec<Vec<u64>>,
    pub(crate) requirements: Vec<(String, serde_json::Value)>,
}

/// The sockets of a document of these rules, as a prior.
pub(crate) fn prior_sockets(extracted: &Extracted) -> Vec<PriorSocket> {
    extracted
        .sockets
        .iter()
        .map(|s| PriorSocket {
            id: s.id.clone(),
            occurrences: s.occurrences.clone(),
            requirements: s
                .requirements
                .iter()
                .map(|r| (r.id.clone(), values(r)))
                .collect(),
        })
        .collect()
}

/// True if `new` states every field of `old`, equal: objects key by key
/// (a key `new` adds is not compared), arrays whole (as long, item by
/// item), anything else equal. Between documents of the same rules this is
/// equality; a field a later version adds is not compared, and one it
/// changes, renames or drops is a change (MF11).
fn same(old: &serde_json::Value, new: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => a
            .iter()
            .all(|(key, x)| b.get(key).is_some_and(|y| same(x, y))),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        _ => old == new,
    }
}

/// What the user decides on: the requirement's values without its
/// touchers, which name each socket's own neighbours (a seat on the same
/// plane is the same requirement in every socket).
fn target_values(requirement: &Requirement) -> String {
    let mut value = serde_json::to_value((
        &requirement.requires,
        requirement.floor_mm,
        &requirement.flags,
    ))
    .unwrap_or_default();
    strip_touchers(&mut value);
    value.to_string()
}

fn strip_touchers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("by");
            map.remove("neighbour");
            map.values_mut().for_each(strip_touchers);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_touchers),
        _ => {}
    }
}

/// Applies `status` (with `note`) to requirement `id` in every socket
/// holding it, or only in `socket`.
pub(crate) fn decide(
    extracted: &Extracted,
    review: &mut Vec<ReviewEntry>,
    id: &str,
    socket: Option<&str>,
    status: ReviewStatus,
    note: Option<&str>,
) -> Result<usize, ReviewError> {
    if status == ReviewStatus::Removed && note.is_none_or(|n| n.trim().is_empty()) {
        return Err(ReviewError::NoteRequired(id.to_owned()));
    }
    if note.is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
        return Err(ReviewError::NoteTooLong);
    }
    if let Some(name) = socket {
        if !extracted.sockets.iter().any(|s| s.id == name) {
            return Err(ReviewError::UnknownSocket(name.to_owned()));
        }
    }
    let holding: Vec<(&Socket, &Requirement)> = extracted
        .sockets
        .iter()
        .filter(|s| socket.is_none_or(|name| s.id == name))
        .filter_map(|s| s.requirements.iter().find(|r| r.id == id).map(|r| (s, r)))
        .collect();
    if holding.is_empty() {
        return Err(ReviewError::UnknownId(id.to_owned()));
    }
    let distinct: BTreeSet<String> = holding.iter().map(|(_, r)| target_values(r)).collect();
    if distinct.len() > 1 {
        return Err(ReviewError::DiffersBetweenSockets {
            id: id.to_owned(),
            sockets: holding.iter().map(|(s, _)| s.id.clone()).collect(),
        });
    }
    for (s, _) in &holding {
        let entry = ReviewEntry {
            socket: s.id.clone(),
            requirement: id.to_owned(),
            status,
            note: note.map(str::to_owned),
        };
        match review
            .iter_mut()
            .find(|e| e.socket == s.id && e.requirement == id)
        {
            Some(existing) => *existing = entry,
            None => review.push(entry),
        }
    }
    sort(review, extracted);
    Ok(holding.len())
}

/// The review in the document's order: by socket, then requirement.
fn sort(review: &mut [ReviewEntry], extracted: &Extracted) {
    let place: BTreeMap<(&str, &str), usize> = extracted
        .sockets
        .iter()
        .flat_map(|s| {
            s.requirements
                .iter()
                .map(move |r| (s.id.as_str(), r.id.as_str()))
        })
        .enumerate()
        .map(|(i, key)| (key, i))
        .collect();
    review.sort_by_key(|e| {
        place
            .get(&(e.socket.as_str(), e.requirement.as_str()))
            .copied()
            .unwrap_or(usize::MAX)
    });
}

/// One old decision, with the requirement it was made on.
struct Old<'a> {
    socket: &'a PriorSocket,
    entry: &'a ReviewEntry,
    values: &'a serde_json::Value,
}

/// 9.5: each old decision carried to a new requirement only when every
/// occurrence of the new socket had that decision, the decisions agree,
/// and the requirement is byte-equal (sources aside); otherwise the new
/// requirement stays unreviewed and the old decision goes stale.
pub(crate) fn carry_over(
    new: &Extracted,
    review: &mut [ReviewEntry],
    old: &[PriorSocket],
    old_review: &[ReviewEntry],
    stale: &mut Vec<StaleReview>,
) -> CarryOver {
    let decided: Vec<Old<'_>> = old_review
        .iter()
        .filter(|e| e.status != ReviewStatus::Unreviewed)
        .filter_map(|entry| {
            let socket = old.iter().find(|s| s.id == entry.socket)?;
            let (_, values) = socket
                .requirements
                .iter()
                .find(|(id, _)| *id == entry.requirement)?;
            Some(Old {
                socket,
                entry,
                values,
            })
        })
        .collect();
    let mut outcome = CarryOver::default();
    // Why each old decision was not carried, if it was not.
    let mut why: BTreeMap<usize, StaleReason> = BTreeMap::new();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for socket in &new.sockets {
        for requirement in &socket.requirements {
            let mine: Vec<(usize, &Old<'_>)> = decided
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    d.entry.requirement == requirement.id
                        && d.socket
                            .occurrences
                            .iter()
                            .any(|o| socket.occurrences.contains(o))
                })
                .collect();
            if mine.is_empty() {
                continue;
            }
            used.extend(mine.iter().map(|(i, _)| *i));
            let every = socket
                .occurrences
                .iter()
                .all(|o| mine.iter().any(|(_, d)| d.socket.occurrences.contains(o)));
            let agree = mine.windows(2).all(|w| {
                w[0].1.entry.status == w[1].1.entry.status && w[0].1.entry.note == w[1].1.entry.note
            });
            let now = values(requirement);
            let unchanged = mine.iter().all(|(_, d)| same(d.values, &now));
            let reason = if !every || !agree {
                Some(StaleReason::OccurrencesDisagree)
            } else if !unchanged {
                Some(StaleReason::RequirementChanged)
            } else {
                None
            };
            match (reason, mine.first()) {
                (None, Some((_, d))) => {
                    if let Some(entry) = review
                        .iter_mut()
                        .find(|e| e.socket == socket.id && e.requirement == requirement.id)
                    {
                        entry.status = d.entry.status;
                        entry.note.clone_from(&d.entry.note);
                    }
                    outcome.carried += 1;
                }
                (Some(reason), _) => {
                    for (i, _) in &mine {
                        why.entry(*i).or_insert(reason);
                    }
                }
                _ => {}
            }
        }
    }
    for (i, d) in decided.iter().enumerate() {
        let reason = if used.contains(&i) {
            why.get(&i).copied()
        } else {
            Some(StaleReason::RequirementGone)
        };
        if let Some(reason) = reason {
            stale.push(StaleReview {
                socket: d.entry.socket.clone(),
                requirement: d.entry.requirement.clone(),
                status: d.entry.status,
                note: d.entry.note.clone(),
                reason,
            });
            outcome.stale += 1;
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::same;
    use serde_json::json;

    /// MF11: every field the older version states is compared, and must
    /// still be there; one the newer adds is not; arrays are compared whole.
    #[test]
    fn values_agree_on_the_fields_both_state() {
        let old = json!([{"kind": "bore", "spans": [{"from_mm": 0.0, "to_mm": 5.0}]}, 0.01, []]);
        let added = json!([{"kind": "bore", "spans": [{"from_mm": 0.0, "to_mm": 5.0, "basis": "cylinder"}]}, 0.01, []]);
        assert!(same(&old, &old));
        assert!(same(&old, &added));
        let moved = json!([{"kind": "bore", "spans": [{"from_mm": 0.0, "to_mm": 5.01}]}, 0.01, []]);
        assert!(!same(&old, &moved));
        let split = json!([{"kind": "bore", "spans": [{"from_mm": 0.0, "to_mm": 2.0}, {"from_mm": 2.0, "to_mm": 5.0}]}, 0.01, []]);
        assert!(!same(&old, &split));
        let flagged = json!([{"kind": "bore", "spans": [{"from_mm": 0.0, "to_mm": 5.0}]}, 0.01, ["seat_unverifiable"]]);
        assert!(!same(&old, &flagged));
        let other = json!([{"kind": "shaft", "spans": [{"from_mm": 0.0, "to_mm": 5.0}]}, 0.01, []]);
        assert!(!same(&old, &other));
        let dropped = json!([{"kind": "bore", "spans": [{"from_mm": 0.0}]}, 0.01, []]);
        assert!(!same(&old, &dropped));
    }
}
